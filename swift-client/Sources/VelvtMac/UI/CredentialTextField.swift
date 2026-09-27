import AppKit
import SwiftUI

/// AppKit-backed credential input avoids the SwiftUI text-input remote view,
/// which faults repeatedly when focus changes in this accessory application.
struct CredentialTextField: NSViewRepresentable {
    let placeholder: String
    @Binding var text: String
    var isSecure = false

    func makeCoordinator() -> Coordinator {
        Coordinator(text: $text)
    }

    func makeNSView(context: Context) -> NSTextField {
        let field: NSTextField = isSecure ? NSSecureTextField() : NSTextField()
        // The same placeholder string, set in the brand face and a quiet
        // paper so it reads as a hint rather than as content.
        field.placeholderAttributedString = VelvtFieldChrome.placeholder(placeholder)
        field.isBezeled = true
        field.bezelStyle = .roundedBezel
        field.focusRingType = .default
        // Brand chrome only. The bezel and the focus ring stay native: they
        // carry the field's text insets and its keyboard-focus trace, and a
        // hand-drawn substitute would change how the field behaves, not just
        // how it looks. What the brand owns is the face of it — Manrope, in
        // paper on the ink ground the credential sheets sit on.
        field.font = VelvtFieldChrome.font
        field.textColor = VelvtFieldChrome.textColor
        field.delegate = context.coordinator
        return field
    }

    func updateNSView(_ field: NSTextField, context: Context) {
        if field.stringValue != text {
            field.stringValue = text
        }
    }

    final class Coordinator: NSObject, NSTextFieldDelegate {
        private var text: Binding<String>

        init(text: Binding<String>) {
            self.text = text
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSTextField else {
                return
            }
            text.wrappedValue = field.stringValue
        }
    }
}

/// The brand tokens this field needs, bridged to the AppKit types a
/// `NSTextField` takes. Nothing new is decided here: the size is the body size
/// from `VelvtType`, the colours are `VelvtInk`, and the face is the bundled
/// Manrope that `VelvtFonts` registers.
private enum VelvtFieldChrome {
    static let size: CGFloat = 13

    /// Manrope Regular, or the system face at the same size when the bundled
    /// resource is unavailable — the same fallback `VelvtFonts` makes.
    static var font: NSFont {
        // Resolving a brand font is what registers the bundled faces with the
        // process, after which the PostScript name resolves for AppKit too.
        _ = VelvtType.body(size)
        return NSFont(name: "Manrope-Regular", size: size)
            ?? .systemFont(ofSize: size)
    }

    static var textColor: NSColor { NSColor(VelvtInk.primaryOnInk) }
    static var placeholderColor: NSColor { NSColor(VelvtInk.tertiaryOnInk) }

    static func placeholder(_ text: String) -> NSAttributedString {
        NSAttributedString(
            string: text,
            attributes: [.font: font, .foregroundColor: placeholderColor]
        )
    }
}
