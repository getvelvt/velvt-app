import AppKit
import SwiftUI
import os

private let remoteTextInputLog = Logger(subsystem: "com.velvt.mac", category: "RemoteTextInput")

/// Gives AppKit's text-input system (the XPC-backed `CursorUIViewService` /
/// `TUINSRemoteViewController`, used for the cursor/IME UI on text fields) a
/// concrete `NSViewController` ancestor for SwiftUI-hosted text fields.
///
/// IMPORTANT: there is no public AppKit override point for "the remote
/// text-input service crashed." `remoteViewControllerDidFailWithError(_:)`
/// does not exist anywhere in the macOS AppKit SDK — it was verified absent
/// from `NSViewController.h` and every other header in the SDK; it belongs
/// to iOS app-extension hosting, not AppKit. The "CLIENT ERROR ... does not
/// override -[private] ..." log line is AppKit's own internal class
/// (`TUINSRemoteViewController`) describing *its own* missing override, not
/// ours — there is no app-facing hook to react to it, and per Apple's own
/// log message AppKit's default behavior is already to log and continue, so
/// the field keeps working without any code on our side.
///
/// What public API *does* let us do: avoid relying purely on SwiftUI's own
/// internal `WindowGroup` hosting (which has no view-controller subclassing
/// point at all) by giving these fields a single, stable `NSViewController`
/// owner, and proactively refresh the active `NSTextInputContext` whenever a
/// field becomes first responder — the documented mechanism for telling the
/// text-input system "the document's coordinates/selection may be stale, requery
/// them" (`NSTextInputContext.invalidateCharacterCoordinates()`). This is the
/// real analogue of "re-establish the text input state" for a fault that has
/// no other public recovery surface.
final class ResilientTextInputHostingController<Content: View>: NSHostingController<Content> {
    override func viewDidAppear() {
        super.viewDidAppear()
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(textDidBeginEditing(_:)),
            name: NSControl.textDidBeginEditingNotification,
            object: nil
        )
    }

    override func viewDidDisappear() {
        super.viewDidDisappear()
        NotificationCenter.default.removeObserver(
            self,
            name: NSControl.textDidBeginEditingNotification,
            object: nil
        )
    }

    @objc private func textDidBeginEditing(_ notification: Notification) {
        remoteTextInputLog.debug("Text field became active; refreshing input context")
        NSTextInputContext.current?.invalidateCharacterCoordinates()
    }
}

/// Wraps SwiftUI text-input content (e.g. the email/password fields in the
/// auth flow) in `ResilientTextInputHostingController` so the fields have a
/// stable `NSViewController` ancestor instead of relying solely on SwiftUI's
/// own internal hosting for `WindowGroup`.
struct TextInputResilientContainer<Content: View>: NSViewControllerRepresentable {
    private let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    func makeNSViewController(context: Context) -> ResilientTextInputHostingController<Content> {
        let controller = ResilientTextInputHostingController(rootView: content)
        // Without this, NSHostingController doesn't propagate SwiftUI's
        // intrinsic size to the surrounding VStack and the fields collapse.
        controller.sizingOptions = [.intrinsicContentSize]
        return controller
    }

    func updateNSViewController(
        _ nsViewController: ResilientTextInputHostingController<Content>,
        context: Context
    ) {
        nsViewController.rootView = content
    }
}
