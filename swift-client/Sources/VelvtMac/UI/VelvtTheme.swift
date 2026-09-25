import AppKit
import SwiftUI

#if canImport(CoreText)
import CoreText
#endif

// =============================================================================
// MARK: - Velvt brand system
// =============================================================================
//
// The single source of visual truth, transcribed from the Velvt brand system
// (Style Guide, 01 / 2026). Nothing here decides behaviour; every value is a
// colour, a metric, a typeface, or a shape.
//
// The guide's four attributes — CALM, CAPABLE, PREMIUM, POSITIVE — are what the
// numbers below are for. Two of its rules constrain this file directly:
//
//   VISUAL   tinted glass · soft folds · clear traces
//   AVOID    guilt · surveillance · gamified pressure
//
// "Tinted glass" is why surfaces are gradients rather than flat fills: sampling
// the guide's own dark cards returns #38232D at one corner and #4F2A3B at
// another, so a flat fill would be a transcription error. "Avoid gamified
// pressure" is why there is no success green, no warning amber, and no progress
// colour anywhere in the palette. Sage is the only affirmative hue the brand
// owns, and it is quiet on purpose.
//
// =============================================================================

// MARK: - Palette

/// The six brand colours, exactly as the guide names and numbers them.
///
/// These six are the whole palette. A new colour is a brand decision, not a
/// view's decision, so views compose from `VelvtSurface` and `VelvtInk` below
/// rather than reaching for a literal.
public enum VelvtPalette {
    /// #A80A43 — carries energy. Primary actions and the one claim per card.
    public static let crimson = Color(hex: 0xA8_0A_43)
    /// #D23871 — emphasis inside running text, and measured values.
    public static let signal = Color(hex: 0xD2_38_71)
    /// #F1C4D3 — the tint behind an experiment the user can decline.
    public static let blush = Color(hex: 0xF1_C4_D3)
    /// #160B11 — the ground. Verified against the guide's own canvas.
    public static let ink = Color(hex: 0x16_0B_11)
    /// #F7F0EA — paper. Card stock, and primary text on ink.
    public static let paper = Color(hex: 0xF7_F0_EA)
    /// #7D9287 — the brand's only affirmative hue. Deliberately undersaturated:
    /// a calm confirmation, never a reward.
    public static let sage = Color(hex: 0x7D_92_87)
}

// MARK: - Surfaces

/// Grounds, cards, and the strokes that separate them.
///
/// Velvt runs in a menu-bar popover over whatever the user is working in, so
/// the ground is always ink and never adapts to system appearance. A coach that
/// changed colour with the OS would read as a system surface; this one is meant
/// to read as a note from something that works for you.
public enum VelvtSurface {
    /// The window ground.
    public static let ground = VelvtPalette.ink

    /// Tinted glass: ink lifted toward crimson, brighter at the top-leading
    /// corner the way the guide's cards are.
    public static let card = LinearGradient(
        colors: [Color(hex: 0x3A_23_30), Color(hex: 0x26_15_1E)],
        startPoint: .topLeading,
        endPoint: .bottomTrailing
    )

    /// A raised tinted surface for a panel sitting on top of a card.
    public static let cardRaised = LinearGradient(
        colors: [Color(hex: 0x4A_2C_3C), Color(hex: 0x33_1E_29)],
        startPoint: .topLeading,
        endPoint: .bottomTrailing
    )

    /// Paper stock, for a message addressed to the person rather than a panel
    /// of data about them. The guide reserves it for exactly that.
    public static let paperCard = LinearGradient(
        colors: [VelvtPalette.paper, Color(hex: 0xEC_E4_DD)],
        startPoint: .top,
        endPoint: .bottom
    )

    /// The tint behind a proposed experiment, on paper.
    public static let blushInset = Color(hex: 0xFA_E9_EE)

    /// Hairlines. "Clear traces" — visible enough to bound a surface, never
    /// heavy enough to box the reader in.
    public static let strokeOnInk = VelvtPalette.paper.opacity(0.10)
    public static let strokeOnPaper = VelvtPalette.ink.opacity(0.10)
    public static let strokeBlush = VelvtPalette.blush
    public static let strokeCrimson = VelvtPalette.crimson.opacity(0.35)

    /// A flat equivalent of `card`, for the few places SwiftUI wants a `Color`
    /// rather than a `ShapeStyle` (menu backgrounds, list row tints).
    public static let cardFlat = Color(hex: 0x30_1C_27)
}

// MARK: - Text

/// Text colours, named by the ground they sit on.
///
/// Reading a colour off the wrong ground is the most common way a restyle goes
/// wrong, so the ground is in the name and there is no ground-agnostic default.
public enum VelvtInk {
    // On the ink ground.
    public static let primaryOnInk = VelvtPalette.paper
    public static let secondaryOnInk = VelvtPalette.paper.opacity(0.68)
    public static let tertiaryOnInk = VelvtPalette.paper.opacity(0.42)
    /// Eyebrows and section labels on ink.
    public static let labelOnInk = VelvtPalette.signal

    // On paper.
    public static let primaryOnPaper = VelvtPalette.ink
    public static let secondaryOnPaper = VelvtPalette.ink.opacity(0.74)
    public static let tertiaryOnPaper = VelvtPalette.ink.opacity(0.50)
    /// Eyebrows and section labels on paper.
    public static let labelOnPaper = VelvtPalette.crimson

    // -------------------------------------------------------------------
    // Emphasis colours are ground-aware, and this is not a style preference.
    // Measured against the two grounds (WCAG 2.1, sRGB):
    //
    //              on ink #160B11      on paper #F7F0EA
    //   crimson        2.56:1 ✗            6.67:1 ✓
    //   signal         4.17:1 ~            4.09:1 ~
    //   sage           5.81:1 ✓            2.94:1 ✗
    //   blush         12.45:1 ✓                 —
    //   paper         17.07:1 ✓                 —
    //   ink                 —             17.07:1 ✓
    //
    // Crimson is the brand's energy and it is *unreadable on ink* — below the
    // 3:1 floor for even large text. Sage inverts the problem. So neither has a
    // ground-agnostic form here: asking for "the measurement colour" without
    // saying which ground is exactly how a palette produces 2.5:1 body text.
    // -------------------------------------------------------------------

    /// A measured value quoted inside running text — "68 MIN", "9 of 12".
    ///
    /// On paper this is crimson, as the guide prints it. On ink crimson is
    /// unreadable, so signal stands in at 4.17:1 — which clears AA for large
    /// bold text but not for small body, which is why `VelvtType.measurement`
    /// floors at 14pt. Measurements are set bold and short by design.
    public static let measurementOnInk = VelvtPalette.signal
    public static let measurementOnPaper = VelvtPalette.crimson

    /// Quiet affirmation. Never a score, never a streak.
    ///
    /// Sage is readable on ink but fails on paper at 2.94:1, so the paper form
    /// is sage carried 25% toward ink — the same hue, clearing AA at 4.59:1.
    public static let affirmativeOnInk = VelvtPalette.sage
    public static let affirmativeOnPaper = Color(hex: 0x63_70_6A)

    /// Ground-agnostic aliases, resolved to the **ink** form because the app
    /// runs on ink everywhere except a paper card. A call site that has not
    /// been given a ground still renders readably; it is only ever wrong on a
    /// paper card, where it is too dark rather than invisible.
    public static let measurement = measurementOnInk
    public static let affirmative = affirmativeOnInk
}

// MARK: - Typography

/// Manrope, in the three weights the guide specifies.
///
/// The guide's scale is printed at document scale — DISPLAY 30/33, HEADING
/// 15/18, BODY 9.5/14, LABEL 6.5/10 — where the second number is the line
/// height. A 9.5pt body would be unreadable in a macOS popover, so the sizes
/// below are set for the screen while the guide's *line-height ratios* (1.10
/// display, 1.20 heading, 1.47 body, 1.54 label) are preserved exactly. The
/// weights are not adapted: ExtraBold, Bold, and Regular are the guide's.
public enum VelvtType {
    /// Ratios lifted from the guide, applied to screen sizes.
    private enum Leading {
        static let display = 1.10
        static let heading = 1.20
        static let body = 1.47
        static let label = 1.54
    }

    public static func display(_ size: CGFloat = 26) -> Font {
        VelvtFonts.font(.extraBold, size: size)
    }

    public static func title(_ size: CGFloat = 18) -> Font {
        VelvtFonts.font(.bold, size: size)
    }

    public static func heading(_ size: CGFloat = 15) -> Font {
        VelvtFonts.font(.bold, size: size)
    }

    public static func body(_ size: CGFloat = 13) -> Font {
        VelvtFonts.font(.regular, size: size)
    }

    public static func bodyEmphasis(_ size: CGFloat = 13) -> Font {
        VelvtFonts.font(.bold, size: size)
    }

    public static func caption(_ size: CGFloat = 11.5) -> Font {
        VelvtFonts.font(.regular, size: size)
    }

    /// The uppercase, letterspaced eyebrow: "PATTERN / MORNING FOCUS".
    public static func label(_ size: CGFloat = 10) -> Font {
        VelvtFonts.font(.bold, size: size)
    }

    /// A measured number quoted in running text.
    ///
    /// Floors at 14pt: on ink this is set in signal at 4.17:1, which is below
    /// the 4.5:1 body-text threshold but clears the 3:1 one for text that is
    /// bold and at least 14pt. The size is part of the contrast guarantee, so
    /// it is enforced here rather than left to each call site.
    public static func measurement(_ size: CGFloat = 14) -> Font {
        VelvtFonts.font(.extraBold, size: max(size, 14))
    }

    // Line spacing is the *extra* space SwiftUI adds between lines, so the
    // guide's ratio has to have the font size subtracted back out.
    public static func displaySpacing(_ size: CGFloat = 26) -> CGFloat {
        size * (Leading.display - 1)
    }
    public static func headingSpacing(_ size: CGFloat = 15) -> CGFloat {
        size * (Leading.heading - 1)
    }
    public static func bodySpacing(_ size: CGFloat = 13) -> CGFloat {
        size * (Leading.body - 1)
    }
    public static func labelSpacing(_ size: CGFloat = 10) -> CGFloat {
        size * (Leading.label - 1)
    }

    /// Tracking for the uppercase label. The guide sets its labels loose enough
    /// to read as a system marker rather than shouting.
    public static let labelTracking: CGFloat = 0.9
}

/// Registers the bundled Manrope faces and resolves them by PostScript name.
///
/// The faces are vendored under `Resources/Fonts` (see `NOTICE.md` there) and
/// registered process-scoped at first use, so no Info.plist entry and no
/// user-level font install is required for the SwiftPM build. If registration
/// fails — a corrupt resource, a sandbox that refuses it — every call falls
/// back to the system face at the same weight. The UI degrades to the wrong
/// typeface, never to a crash and never to an unreadable size.
public enum VelvtFonts {
    public enum Weight {
        case regular, bold, extraBold

        var postScriptName: String {
            switch self {
            case .regular: return "Manrope-Regular"
            case .bold: return "Manrope-Bold"
            case .extraBold: return "Manrope-ExtraBold"
            }
        }

        var systemFallback: Font.Weight {
            switch self {
            case .regular: return .regular
            case .bold: return .bold
            case .extraBold: return .heavy
            }
        }
    }

    /// `true` once the bundled faces are available to this process.
    public private(set) static var isRegistered = false

    private static let registrationOnce: Void = {
        #if canImport(CoreText)
        let names = ["Manrope-Regular", "Manrope-Bold", "Manrope-ExtraBold"]
        var registeredAny = false
        for bundle in candidateBundles {
            for name in names {
                guard let url = bundle.url(forResource: name, withExtension: "ttf")
                    ?? bundle.url(
                        forResource: name,
                        withExtension: "ttf",
                        subdirectory: "Fonts"
                    )
                else { continue }
                // `.process` scope keeps the faces out of the user's font book;
                // a coaching app has no business installing fonts system-wide.
                if CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil) {
                    registeredAny = true
                }
            }
            if registeredAny { break }
        }
        VelvtFonts.isRegistered = registeredAny
        #endif
    }()

    private static var candidateBundles: [Bundle] {
        var bundles: [Bundle] = []
        #if SWIFT_PACKAGE
        bundles.append(Bundle.module)
        #endif
        bundles.append(Bundle.main)
        bundles.append(Bundle(for: BundleToken.self))
        return bundles
    }

    private final class BundleToken {}

    /// Manrope at `weight`/`size`, or the system face at a matching weight when
    /// the bundled resource is unavailable.
    public static func font(_ weight: Weight, size: CGFloat) -> Font {
        _ = registrationOnce
        if isRegistered {
            return .custom(weight.postScriptName, size: size)
        }
        return .system(size: size, weight: weight.systemFallback)
    }
}

// MARK: - Metrics

/// Radii, spacing, and stroke widths.
///
/// "Soft folds": the guide's corners are generous and consistent, and a card
/// never has a tighter corner than the panel inside it.
public enum VelvtMetrics {
    public static let cardRadius: CGFloat = 18
    public static let panelRadius: CGFloat = 14
    public static let controlRadius: CGFloat = 10
    public static let chipRadius: CGFloat = 7

    public static let hairline: CGFloat = 1

    public static let spaceXS: CGFloat = 4
    public static let spaceSM: CGFloat = 8
    public static let spaceMD: CGFloat = 12
    public static let spaceLG: CGFloat = 18
    public static let spaceXL: CGFloat = 26

    /// Interior padding for a card and for a panel nested in one.
    public static let cardPadding: CGFloat = 16
    public static let panelPadding: CGFloat = 13
}

// MARK: - Components

/// The uppercase eyebrow above a card's claim: `PATTERN / MORNING FOCUS`.
///
/// Two halves so the slash reads as a system marker rather than punctuation in
/// a sentence. `detail` is optional because not every card is qualified.
public struct VelvtEyebrow: View {
    private let kind: String
    private let detail: String?
    private let onPaper: Bool

    public init(_ kind: String, detail: String? = nil, onPaper: Bool = false) {
        self.kind = kind
        self.detail = detail
        self.onPaper = onPaper
    }

    public var body: some View {
        Text(text)
            .font(VelvtType.label())
            .tracking(VelvtType.labelTracking)
            .lineSpacing(VelvtType.labelSpacing())
            .foregroundStyle(onPaper ? VelvtInk.labelOnPaper : VelvtInk.labelOnInk)
            .accessibilityLabel(accessibleText)
    }

    private var text: String {
        guard let detail else { return kind.uppercased() }
        return "\(kind.uppercased()) / \(detail.uppercased())"
    }

    /// VoiceOver should not read the slash as "slash".
    private var accessibleText: String {
        guard let detail else { return kind }
        return "\(kind), \(detail)"
    }
}

/// A tinted-glass card on the ink ground.
public struct VelvtCard<Content: View>: View {
    private let padding: CGFloat
    private let content: Content

    public init(padding: CGFloat = VelvtMetrics.cardPadding, @ViewBuilder content: () -> Content) {
        self.padding = padding
        self.content = content()
    }

    public var body: some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: VelvtMetrics.cardRadius, style: .continuous)
                    .fill(VelvtSurface.card)
            )
            .overlay(
                RoundedRectangle(cornerRadius: VelvtMetrics.cardRadius, style: .continuous)
                    .strokeBorder(VelvtSurface.strokeOnInk, lineWidth: VelvtMetrics.hairline)
            )
    }
}

/// A paper card — reserved for a message addressed to the person.
public struct VelvtPaperCard<Content: View>: View {
    private let padding: CGFloat
    private let content: Content

    public init(padding: CGFloat = VelvtMetrics.cardPadding, @ViewBuilder content: () -> Content) {
        self.padding = padding
        self.content = content()
    }

    public var body: some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: VelvtMetrics.cardRadius, style: .continuous)
                    .fill(VelvtSurface.paperCard)
            )
    }
}

/// The blush panel that proposes an experiment. Tinted, bordered, and always
/// declinable — the guide's "small experiment you control".
public struct VelvtInsetPanel<Content: View>: View {
    private let content: Content

    public init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    public var body: some View {
        content
            .padding(VelvtMetrics.panelPadding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: VelvtMetrics.panelRadius, style: .continuous)
                    .fill(VelvtSurface.blushInset)
            )
            .overlay(
                RoundedRectangle(cornerRadius: VelvtMetrics.panelRadius, style: .continuous)
                    .strokeBorder(VelvtSurface.strokeBlush, lineWidth: VelvtMetrics.hairline)
            )
    }
}

// MARK: - Buttons

/// The one crimson action on a surface.
///
/// The guide sets its primary buttons uppercase, but this style does **not**
/// uppercase by default. Action labels on the drift card and the recovery
/// prompt are authored in Rust beside the evidence that justifies them and are
/// documented as rendered verbatim — `.textCase(.uppercase)` would silently
/// rewrite service copy, which is a content change wearing a style's clothes.
/// Pass `uppercase: true` at a call site whose label is a static Swift literal.
public struct VelvtPrimaryButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled
    private let uppercase: Bool

    public init(uppercase: Bool = false) {
        self.uppercase = uppercase
    }

    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(VelvtType.label(11))
            .tracking(VelvtType.labelTracking)
            .textCase(uppercase ? .uppercase : nil)
            .foregroundStyle(VelvtPalette.paper)
            .padding(.horizontal, VelvtMetrics.spaceLG)
            .padding(.vertical, 9)
            .background(
                RoundedRectangle(cornerRadius: VelvtMetrics.controlRadius, style: .continuous)
                    .fill(VelvtPalette.crimson)
            )
            .opacity(opacity(for: configuration))
            .contentShape(
                RoundedRectangle(cornerRadius: VelvtMetrics.controlRadius, style: .continuous)
            )
    }

    private func opacity(for configuration: Configuration) -> Double {
        if !isEnabled { return 0.4 }
        return configuration.isPressed ? 0.82 : 1
    }
}

/// The decline. Same size and weight of presence as the primary, in paper —
/// "I'm fine..." must never look harder to press than "Start experiment".
public struct VelvtSecondaryButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled
    private let onPaper: Bool

    public init(onPaper: Bool = true) {
        self.onPaper = onPaper
    }

    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(VelvtType.body(12))
            .foregroundStyle(onPaper ? VelvtInk.primaryOnPaper : VelvtInk.primaryOnInk)
            .padding(.horizontal, VelvtMetrics.spaceLG)
            .padding(.vertical, 9)
            .background(
                RoundedRectangle(cornerRadius: VelvtMetrics.controlRadius, style: .continuous)
                    .fill(onPaper ? VelvtPalette.paper : VelvtPalette.paper.opacity(0.10))
            )
            .overlay(
                RoundedRectangle(cornerRadius: VelvtMetrics.controlRadius, style: .continuous)
                    .strokeBorder(
                        onPaper ? VelvtSurface.strokeOnPaper : VelvtSurface.strokeOnInk,
                        lineWidth: VelvtMetrics.hairline
                    )
            )
            .opacity(opacity(for: configuration))
            .contentShape(
                RoundedRectangle(cornerRadius: VelvtMetrics.controlRadius, style: .continuous)
            )
    }

    private func opacity(for configuration: Configuration) -> Double {
        if !isEnabled { return 0.4 }
        return configuration.isPressed ? 0.82 : 1
    }
}

/// A destructive action — End, Delete Account, Undo.
///
/// The brand owns no danger colour, and inventing a red would put a seventh
/// hue on the palette. A custom `ButtonStyle` also discards SwiftUI's
/// `role: .destructive` presentation, so without this these actions would
/// render identically to the calm ones beside them and lose their warning
/// entirely. This keeps them distinguishable the way the palette allows:
/// the brand's energy colour, on a bordered but unfilled ground, so it reads
/// as "different from its neighbours" without shouting.
public struct VelvtDestructiveButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled
    private let onPaper: Bool

    public init(onPaper: Bool = false) {
        self.onPaper = onPaper
    }

    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(VelvtType.bodyEmphasis(12))
            // Crimson is 2.56:1 on ink, so the ink form uses signal.
            .foregroundStyle(onPaper ? VelvtPalette.crimson : VelvtPalette.signal)
            .padding(.horizontal, VelvtMetrics.spaceLG)
            .padding(.vertical, 9)
            .background(
                RoundedRectangle(cornerRadius: VelvtMetrics.controlRadius, style: .continuous)
                    .fill(Color.clear)
            )
            .overlay(
                RoundedRectangle(cornerRadius: VelvtMetrics.controlRadius, style: .continuous)
                    .strokeBorder(
                        (onPaper ? VelvtPalette.crimson : VelvtPalette.signal).opacity(0.45),
                        lineWidth: VelvtMetrics.hairline
                    )
            )
            .opacity(isEnabled ? (configuration.isPressed ? 0.82 : 1) : 0.4)
            .contentShape(
                RoundedRectangle(cornerRadius: VelvtMetrics.controlRadius, style: .continuous)
            )
    }
}

/// Type-erases a `ButtonStyle` so a call site can pick between two styles by a
/// value without duplicating the button. SwiftUI has no built-in eraser for
/// `ButtonStyle`, and the alternative — branching the whole `Button` — would
/// duplicate its action closure, which is exactly the kind of change a restyle
/// must not make.
public struct AnyButtonStyle: ButtonStyle {
    private let make: (Configuration) -> AnyView

    public init<S: ButtonStyle>(_ style: S) {
        make = { configuration in AnyView(style.makeBody(configuration: configuration)) }
    }

    public func makeBody(configuration: Configuration) -> some View {
        make(configuration)
    }
}

/// A text-weight action for tertiary choices, so a row of options never grows
/// three filled buttons competing for the same tap.
public struct VelvtQuietButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled
    private let onPaper: Bool

    public init(onPaper: Bool = false) {
        self.onPaper = onPaper
    }

    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(VelvtType.body(12))
            .foregroundStyle(onPaper ? VelvtInk.secondaryOnPaper : VelvtInk.secondaryOnInk)
            .padding(.horizontal, VelvtMetrics.spaceMD)
            .padding(.vertical, 6)
            .opacity(isEnabled ? (configuration.isPressed ? 0.6 : 1) : 0.4)
            .contentShape(Rectangle())
    }
}

// MARK: - Text helpers

extension View {
    /// Body copy at the guide's line-height ratio.
    public func velvtBody(_ size: CGFloat = 13, onPaper: Bool = false) -> some View {
        font(VelvtType.body(size))
            .lineSpacing(VelvtType.bodySpacing(size))
            .foregroundStyle(onPaper ? VelvtInk.secondaryOnPaper : VelvtInk.secondaryOnInk)
    }

    /// A heading at the guide's line-height ratio.
    public func velvtHeading(_ size: CGFloat = 15, onPaper: Bool = false) -> some View {
        font(VelvtType.heading(size))
            .lineSpacing(VelvtType.headingSpacing(size))
            .foregroundStyle(onPaper ? VelvtInk.primaryOnPaper : VelvtInk.primaryOnInk)
    }

    /// The card's one claim.
    public func velvtDisplay(_ size: CGFloat = 26, onPaper: Bool = false) -> some View {
        font(VelvtType.display(size))
            .lineSpacing(VelvtType.displaySpacing(size))
            .foregroundStyle(onPaper ? VelvtInk.primaryOnPaper : VelvtInk.primaryOnInk)
    }
}

// MARK: - Categorical colours

/// Colours for activity categories in history and charts.
///
/// **An extension, not a guide rule.** The brand system defines six colours and
/// no categorical ramp, so this derives one from those six rather than
/// inventing hues: crimson and signal anchor it, sage and blush carry the
/// middle, and the tail is paper at decreasing opacity. Categories are a
/// neutral fact about a day, so the ramp descends into neutrals instead of
/// cycling through a rainbow that would imply some categories are better.
public enum VelvtCategoryRamp {
    public static let colors: [Color] = [
        VelvtPalette.crimson,
        VelvtPalette.signal,
        VelvtPalette.sage,
        VelvtPalette.blush,
        VelvtPalette.paper.opacity(0.55),
        VelvtPalette.signal.opacity(0.45),
        VelvtPalette.sage.opacity(0.55),
        VelvtPalette.paper.opacity(0.30),
    ]

    /// Anything beyond the ramp, and anything unclassified.
    public static let unmatched = VelvtPalette.paper.opacity(0.18)

    public static func color(at index: Int) -> Color {
        guard index >= 0, !colors.isEmpty else { return unmatched }
        return colors[index % colors.count]
    }

    /// The text colour to draw *on top of* a ramp swatch.
    ///
    /// The ramp spans crimson (very dark) to paper@0.30 (very light), so a
    /// single fixed label colour is unreadable at one end whichever end you
    /// pick — ink on crimson is 1.4:1. This resolves the swatch against the ink
    /// ground it is composited over, then returns whichever of paper/ink has
    /// the better contrast against it. Computed rather than tabulated so it
    /// stays correct if the ramp changes.
    public static func legibleForeground(on swatch: Color) -> Color {
        let composited = NSColor(swatch).blended(
            withFraction: 0,
            of: NSColor(VelvtPalette.ink)
        ) ?? NSColor(swatch)
        guard let srgb = composited.usingColorSpace(.sRGB) else {
            return VelvtPalette.paper
        }
        // Alpha-composite over ink, since the translucent ramp entries sit on it.
        let a = srgb.alphaComponent
        let ink = NSColor(VelvtPalette.ink).usingColorSpace(.sRGB)
        let ir = ink?.redComponent ?? 0
        let ig = ink?.greenComponent ?? 0
        let ib = ink?.blueComponent ?? 0
        let r = srgb.redComponent * a + ir * (1 - a)
        let g = srgb.greenComponent * a + ig * (1 - a)
        let b = srgb.blueComponent * a + ib * (1 - a)

        func channel(_ c: Double) -> Double {
            c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4)
        }
        let luminance = 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
        // 0.179 is the luminance at which white and black contrast equally.
        return luminance > 0.179 ? VelvtPalette.ink : VelvtPalette.paper
    }
}

// MARK: - Hex

extension Color {
    /// A colour from a `0xRRGGBB` literal, so brand values can be written the
    /// way the guide prints them and diffed against it by eye.
    init(hex: UInt32, opacity: Double = 1) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: opacity
        )
    }
}

// =============================================================================
// MARK: - Legacy token bridge
// =============================================================================
//
// The pre-brand tokens, redefined on top of the palette above. They exist so an
// un-migrated call site renders in brand colours rather than the old ones — a
// restyle that half-lands is worse than one that lands everywhere at once, and
// these keep the floor at "correct colour, wrong token name".
//
// New code should use `VelvtPalette` / `VelvtSurface` / `VelvtInk` directly.
// These are deliberately not marked `@available(*, deprecated)`: the build runs
// warnings-clean and a wall of deprecation noise would hide a real warning.
// =============================================================================

extension Color {
    /// Superseded by `VelvtSurface.cardFlat`.
    public static let velvtSurface = VelvtSurface.cardFlat
    /// Superseded by `VelvtInk.primaryOnInk`.
    public static let velvtText = VelvtInk.primaryOnInk
    /// Superseded by `VelvtInk.tertiaryOnInk`.
    public static let velvtMuted = VelvtInk.tertiaryOnInk
    /// Superseded by `VelvtSurface.cardFlat`.
    public static let velvtPanel = Color(hex: 0x2A_18_22)
    /// Superseded by `VelvtSurface.cardRaised`.
    public static let velvtPanelHighlight = Color(hex: 0x3A_23_30)
    /// Superseded by `VelvtPalette.crimson`. The old value was #B20D53, four
    /// points off the brand crimson; this corrects it.
    public static let velvtPink = VelvtPalette.crimson
    /// Superseded by `VelvtInk.affirmative`. The old value was a saturated
    /// green the brand does not own.
    public static let velvtGreen = VelvtPalette.sage
    /// Superseded by `VelvtPalette.signal`. The brand has no blue.
    public static let velvtBlue = VelvtPalette.signal
}
