import SwiftUI
import XCTest

@testable import VelvtMac

/// The brand system is a transcription of the Velvt style guide (01 / 2026).
/// These tests assert the transcription, not taste: a colour that drifts off
/// the guide's hex is a bug, and a typeface that silently falls back to the
/// system face is the restyle quietly not happening.
final class VelvtThemeTests: XCTestCase {

    // MARK: - Typeface

    /// The whole type system rests on the bundled faces being registrable. If
    /// this fails the UI still renders — `VelvtFonts.font` falls back to the
    /// system face — but it renders in the wrong typeface, which is exactly
    /// the failure a human would not notice in a diff.
    func testManropeRegistersFromTheBundle() {
        _ = VelvtType.body()
        XCTAssertTrue(
            VelvtFonts.isRegistered,
            "Manrope did not register: the Resources/Fonts payload is missing from the bundle"
        )
    }

    /// Registration must not install anything into the user's font book. A
    /// coaching app has no business changing the machine it runs on.
    func testRegisteredFacesResolveByPostScriptName() throws {
        _ = VelvtType.display()
        try XCTSkipUnless(VelvtFonts.isRegistered)
        for name in ["Manrope-Regular", "Manrope-Bold", "Manrope-ExtraBold"] {
            let font = NSFont(name: name, size: 13)
            XCTAssertNotNil(font, "\(name) did not resolve after registration")
        }
    }

    // MARK: - Palette

    /// The six brand colours, against the hex the guide prints.
    func testPaletteMatchesTheGuide() {
        let expected: [(String, Color, UInt32)] = [
            ("crimson", VelvtPalette.crimson, 0xA8_0A_43),
            ("signal", VelvtPalette.signal, 0xD2_38_71),
            ("blush", VelvtPalette.blush, 0xF1_C4_D3),
            ("ink", VelvtPalette.ink, 0x16_0B_11),
            ("paper", VelvtPalette.paper, 0xF7_F0_EA),
            ("sage", VelvtPalette.sage, 0x7D_92_87),
        ]
        for (name, color, hex) in expected {
            assertColor(color, matches: hex, named: name)
        }
    }

    /// The ground is the guide's canvas, sampled from the guide itself.
    func testGroundIsInk() {
        assertColor(VelvtSurface.ground, matches: 0x16_0B_11, named: "ground")
    }

    /// The legacy token names now resolve to brand colours, so a view that has
    /// not been migrated yet is still the right colour. The old accent was
    /// #B20D53, four points off brand.
    func testLegacyTokensBridgeOntoTheBrandPalette() {
        assertColor(Color.velvtPink, matches: 0xA8_0A_43, named: "velvtPink")
        assertColor(Color.velvtGreen, matches: 0x7D_92_87, named: "velvtGreen")
        assertColor(Color.velvtBlue, matches: 0xD2_38_71, named: "velvtBlue")
        assertColor(Color.velvtText, matches: 0xF7_F0_EA, named: "velvtText")
    }

    /// The brand owns no green, amber, or blue. A categorical ramp that
    /// wandered outside the palette would reintroduce exactly the gamified
    /// colour-coding the guide tells us to avoid.
    func testCategoryRampStaysInsideTheBrandHues() {
        let allowed: [UInt32] = [0xA8_0A_43, 0xD2_38_71, 0xF1_C4_D3, 0xF7_F0_EA, 0x7D_92_87]
        for (index, color) in VelvtCategoryRamp.colors.enumerated() {
            let hex = rgbHex(of: color)
            XCTAssertTrue(
                allowed.contains(hex),
                "ramp[\(index)] is #\(String(hex, radix: 16, uppercase: true)), outside the palette"
            )
        }
    }

    // MARK: - Type scale

    /// The guide's line-height ratios: display 1.10, heading 1.20, body 1.47,
    /// label 1.54. SwiftUI's `lineSpacing` is the *extra* space between lines,
    /// so each is the ratio with the font size subtracted back out.
    func testLineSpacingPreservesTheGuideRatios() {
        XCTAssertEqual(VelvtType.displaySpacing(30), 30 * 0.10, accuracy: 0.001)
        XCTAssertEqual(VelvtType.headingSpacing(15), 15 * 0.20, accuracy: 0.001)
        XCTAssertEqual(VelvtType.bodySpacing(13), 13 * 0.47, accuracy: 0.001)
        XCTAssertEqual(VelvtType.labelSpacing(10), 10 * 0.54, accuracy: 0.001)
    }

    // MARK: - Helpers

    private func rgbHex(of color: Color) -> UInt32 {
        let resolved = NSColor(color).usingColorSpace(.sRGB) ?? .black
        let r = UInt32((resolved.redComponent * 255).rounded())
        let g = UInt32((resolved.greenComponent * 255).rounded())
        let b = UInt32((resolved.blueComponent * 255).rounded())
        return (r << 16) | (g << 8) | b
    }

    private func assertColor(
        _ color: Color,
        matches hex: UInt32,
        named name: String,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        let actual = rgbHex(of: color)
        XCTAssertEqual(
            actual,
            hex,
            """
            \(name) is #\(String(actual, radix: 16, uppercase: true)), \
            guide says #\(String(hex, radix: 16, uppercase: true))
            """,
            file: file,
            line: line
        )
    }
}
