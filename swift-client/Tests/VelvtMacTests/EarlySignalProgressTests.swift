import XCTest

@testable import VelvtMac

/// The early-signal gate needs 60 seconds of *readable* activity. When it has
/// not cleared, there are two different reasons and they need opposite advice:
/// you have barely started (wait), or you have worked for an hour in apps Velvt
/// cannot read (waiting will never help — teach it).
///
/// Getting this wrong is the "it says 60 seconds every time I open it" report:
/// a countdown that cannot move, with nothing to act on.
final class EarlySignalProgressTests: XCTestCase {

  private func signal(observed: Int, events: Int) -> LocalEarlySignal {
    LocalEarlySignal(
      status: .insufficientEvidence,
      observedFrom: nil,
      observedThrough: Date(timeIntervalSince1970: 1_800_000_000),
      observedSeconds: observed,
      requiredSeconds: max(0, 60 - observed),
      evidenceEventCount: events,
      focusedSeconds: 0,
      meaningfulSwitchCount: 0,
      longestUninterruptedSeconds: 0,
      observation: nil,
      suggestedAction: nil,
      actionMinutes: 10
    )
  }

  /// The regression. One readable second used to disable the explanation and
  /// put the user back on a countdown that would not move.
  func testASingleReadableSecondDoesNotHideTheRealReason() {
    let text = EarlySignalProgressView.progressText(
      signal(observed: 1, events: 40),
      totalObservedSeconds: 3_600
    )
    XCTAssertTrue(
      text.contains("cannot categorize"),
      "an hour of unreadable activity must not be reported as a countdown: \(text)"
    )
  }

  /// A genuinely new session still gets the countdown — waiting really is the
  /// answer there, and telling that user to go configure something would be
  /// wrong.
  func testAQuietStartStillGetsTheCountdown() {
    let text = EarlySignalProgressView.progressText(
      signal(observed: 10, events: 2),
      totalObservedSeconds: 12
    )
    XCTAssertTrue(text.contains("more seconds"), text)
    XCTAssertFalse(text.contains("cannot categorize"), text)
  }

  /// Everything readable and the bar cleared: neither explanation applies.
  func testNothingToExplainOnceTheBarIsCleared() {
    let text = EarlySignalProgressView.progressText(
      signal(observed: 90, events: 20),
      totalObservedSeconds: 90
    )
    XCTAssertFalse(text.contains("more seconds"), text)
    XCTAssertFalse(text.contains("cannot categorize"), text)
  }

  /// Missing daily context must not invent an accusation.
  func testAbsentTotalFallsBackToTheCountdown() {
    let text = EarlySignalProgressView.progressText(
      signal(observed: 5, events: 30),
      totalObservedSeconds: nil
    )
    XCTAssertTrue(text.contains("more seconds"), text)
  }
}
