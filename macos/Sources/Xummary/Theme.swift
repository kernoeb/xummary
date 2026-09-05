import SwiftUI

/// Colours for both appearances, kept in code so the app needs no asset catalog.
struct Theme {
    let background: Color
    let text: Color
    let dim: Color
    let accent: Color
    let rule: Color
    /// How much of `background` to wash over the blur. Dark needs more: light
    /// text over a bright wallpaper is the case that breaks first.
    let glassWash: Double

    static func of(_ scheme: ColorScheme) -> Theme {
        scheme == .dark ? .dark : .light
    }

    static let dark = Theme(
        background: Color(hex: 0x0F1115),
        text: Color(hex: 0xE6E8EC),
        dim: Color(hex: 0x8A909B),
        accent: Color(hex: 0x7AA2F7),
        rule: Color(hex: 0x232733),
        glassWash: 0.62
    )

    static let light = Theme(
        background: Color(hex: 0xFBFBFD),
        text: Color(hex: 0x14161A),
        dim: Color(hex: 0x6B7280),
        accent: Color(hex: 0x2B5FD9),
        rule: Color(hex: 0xE7E9EF),
        glassWash: 0.45
    )
}

extension Color {
    init(hex: UInt32) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: 1
        )
    }
}
