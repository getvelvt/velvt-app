//! The drift offer's timing over a real socket, against the real
//! `velvt-service` binary, with its own HOME, database and socket.
//!
//! `drift_offer_in_progress.rs` pins the decision through the router in
//! process. This drives the shipped wire end to end the way the Swift client
//! does since protocol 32 — a dwell reported in progress when it begins and
//! closed when it ends — and checks what arrives on the socket and when: the
//! `work_block_state` carrying `active_intervention` is pushed right after the
//! away dwell's in-progress report, nothing withdraws it while the person is
//! away, and the return's in-progress report is what withdraws it.

use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use uuid::Uuid;
use velvt_shared_types::{
    ClientHello, ClientMessage, RawEvent, RequestDemotionState, ServerMessage, StartWorkBlock,
    WorkBlockIntensity, WorkBlockPurpose, WorkBlockSnapshot, PROTOCOL_VERSION,
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(10);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        // Short and under /tmp: a Unix socket path is capped near 104 bytes.
        let path = PathBuf::from(format!(
            "/tmp/velvt-live-{}-{}",
            std::process::id(),
            &Uuid::new_v4().simple().to_string()[..8]
        ));
        fs::create_dir(&path).expect("scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The helper, killed and reaped when the test ends however it ends.
struct Helper(Child);

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn filesystem_sockets_available(scratch: &Scratch) -> bool {
    match UnixListener::bind(scratch.0.join("preflight.sock")) {
        Ok(_) => {
            let _ = fs::remove_file(scratch.0.join("preflight.sock"));
            true
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
            ) =>
        {
            eprintln!("skipping live helper test: filesystem sockets are unavailable");
            false
        }
        Err(error) => panic!("failed to bind preflight socket: {error}"),
    }
}

fn spawn_helper(scratch: &Scratch) -> (Helper, PathBuf) {
    let socket = scratch.0.join("s.sock");
    let mut command = Command::new(env!("CARGO_BIN_EXE_velvt-service"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("VELVT_") {
            command.env_remove(key);
        }
    }
    command
        .env("HOME", &scratch.0)
        .env("VELVT_LOG_LEVEL", "warn")
        .env("VELVT_DATABASE_PATH", scratch.0.join("velvt.sqlite3"))
        .env("VELVT_IPC_SOCKET_PATH", &socket)
        .env(
            "VELVT_ABSTRACTION_TAXONOMY_PATH",
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources/abstraction-taxonomy-mvp-1.json"),
        )
        // Nothing listens on port 9, so no request leaves this machine.
        .env("VELVT_API_BASE_URL", "http://127.0.0.1:9")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let helper = Helper(command.spawn().expect("spawn velvt-service"));
    (helper, socket)
}

struct Connection {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Connection {
    fn open(helper: &mut Helper, socket: &PathBuf) -> Self {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let stream = loop {
            if let Ok(stream) = UnixStream::connect(socket) {
                break stream;
            }
            if let Some(status) = helper.0.try_wait().unwrap() {
                panic!("velvt-service exited before listening: {status}");
            }
            assert!(Instant::now() < deadline, "velvt-service never listened");
            thread::sleep(Duration::from_millis(20));
        };
        stream.set_read_timeout(Some(READ_TIMEOUT)).unwrap();
        Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
        }
    }

    fn send(&mut self, message: &ClientMessage) {
        let mut frame = serde_json::to_vec(message).unwrap();
        frame.push(b'\n');
        self.writer.write_all(&frame).unwrap();
    }

    fn receive(&mut self) -> ServerMessage {
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .expect("a frame within the read timeout");
        assert!(read > 0, "velvt-service closed the connection");
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("undecodable frame: {error}"))
    }

    fn receive_until<T>(&mut self, mut pick: impl FnMut(ServerMessage) -> Option<T>) -> T {
        loop {
            if let Some(found) = pick(self.receive()) {
                return found;
            }
        }
    }

    fn handshake(&mut self) {
        let ServerMessage::ServerHello(hello) = self.receive() else {
            panic!("the first frame is server_hello");
        };
        assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
        assert_eq!(PROTOCOL_VERSION, 32);
        self.send(&ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "live-helper-test".into(),
        }));
        self.receive_until(|message| {
            matches!(message, ServerMessage::Acknowledged(_)).then_some(())
        });
    }

    /// Sends one report and returns every `work_block_state` it caused.
    ///
    /// The helper answers a connection's frames in order and drains its push
    /// queue before it reads the next one, so whatever a report pushed is on
    /// the socket between that report's ack and the answer to the request
    /// sent right behind it.
    fn report(&mut self, event: RawEvent) -> Vec<WorkBlockSnapshot> {
        let event_id = event.event_id;
        self.send(&ClientMessage::RawEvent(event));
        self.send(&ClientMessage::RequestDemotionState(
            RequestDemotionState {},
        ));
        self.receive_until(|message| match message {
            ServerMessage::RawEventAck(ack) if ack.event_id == event_id => Some(()),
            _ => None,
        });
        let mut pushed = Vec::new();
        loop {
            match self.receive() {
                ServerMessage::DemotionState(_) => return pushed,
                ServerMessage::WorkBlockState(snapshot) => pushed.push(snapshot),
                _ => {}
            }
        }
    }
}

fn dwell(
    started_at: DateTime<Utc>,
    app: &str,
    from: i64,
    until: i64,
    in_progress: bool,
) -> RawEvent {
    RawEvent {
        event_id: Uuid::new_v4(),
        occurred_at: started_at + ChronoDuration::seconds(from),
        duration_seconds: if in_progress {
            0
        } else {
            u64::try_from(until - from).unwrap()
        },
        app_name: app.into(),
        window_title: format!("window {from}"),
        bundle_id: None,
        declared_app_category: None,
        document_type_ids: Vec::new(),
        focused_document_url: None,
        in_progress,
    }
}

#[test]
fn a_live_helper_pushes_the_offer_while_the_person_is_away_and_withdraws_it_on_return() {
    let scratch = Scratch::new();
    if !filesystem_sockets_available(&scratch) {
        return;
    }
    let (mut helper, socket) = spawn_helper(&scratch);
    let mut connection = Connection::open(&mut helper, &socket);
    connection.handshake();

    connection.send(&ClientMessage::StartWorkBlock(StartWorkBlock {
        intention: None,
        planned_duration_seconds: 25 * 60,
        purpose: Some(WorkBlockPurpose::DeepWork),
        intensity: WorkBlockIntensity::Medium,
        invitation_id: None,
    }));
    let started = connection.receive_until(|message| match message {
        ServerMessage::WorkBlockState(snapshot) if snapshot.block_id.is_some() => Some(snapshot),
        _ => None,
    });
    // The helper decides at each dwell's `occurred_at`, so the block's first
    // minutes are replayed by stamping reports after its start rather than
    // by waiting for them.
    let started_at = started.started_at.expect("a started block has a start");

    // Anchor, then three departures inside ten minutes after the warm-up.
    // Every dwell is reported in progress when it begins and closed, by the
    // client, when the next one begins.
    let timeline = [
        ("Xcode", 10, 200),
        ("Slack", 200, 215),
        ("Xcode", 215, 260),
        ("Slack", 260, 275),
        ("Xcode", 275, 320),
    ];
    for (index, (app, from, until)) in timeline.iter().enumerate() {
        if index > 0 {
            let (previous, previous_from, previous_until) = timeline[index - 1];
            connection.report(dwell(
                started_at,
                previous,
                previous_from,
                previous_until,
                false,
            ));
        }
        let pushed = connection.report(dwell(started_at, app, *from, *until, true));
        assert!(
            pushed
                .iter()
                .all(|snapshot| snapshot.active_intervention.is_none()),
            "no offer before the third departure"
        );
    }

    // The third departure. The person has just arrived in the away app.
    connection.report(dwell(started_at, "Xcode", 275, 320, false));
    let arrived_away = connection.report(dwell(started_at, "Slack", 320, 368, true));
    let offer = arrived_away
        .iter()
        .find_map(|snapshot| snapshot.active_intervention.clone())
        .expect("the offer is pushed as the person arrives in the away app");
    assert_eq!(
        offer.offered_at.timestamp(),
        (started_at + ChronoDuration::seconds(320)).timestamp(),
        "stamped at the departure, as the gate always stamped it"
    );
    assert!(!offer.title.is_empty() && !offer.body.is_empty());

    // Leaving the away app: its closed report changes nothing on the socket.
    let left_away = connection.report(dwell(started_at, "Slack", 320, 368, false));
    assert!(
        left_away.is_empty(),
        "the away dwell's closed report pushes nothing: {left_away:?}"
    );

    // Back at the anchor. This report, and not an earlier one, withdraws it.
    let returned = connection.report(dwell(started_at, "Xcode", 368, 400, true));
    let last = returned
        .last()
        .expect("the return pushes the resolved state");
    assert!(
        last.active_intervention.is_none(),
        "the return withdraws the offer"
    );
}
