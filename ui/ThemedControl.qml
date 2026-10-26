import QtQuick.Controls

// Bind each role, so live palette changes propagate through Qt's value palette.
Control {
    palette {
        window: Theme.background; windowText: Theme.foreground
        base: Theme.surface; alternateBase: Theme.alternate; text: Theme.foreground
        button: Theme.button; buttonText: Theme.foreground
        highlight: Theme.accent; highlightedText: Theme.accentText
        mid: Theme.border; light: Theme.border; dark: Theme.background
        toolTipBase: Theme.surface; toolTipText: Theme.foreground
        placeholderText: Theme.muted; accent: Theme.accent
        disabled { text: Theme.muted; windowText: Theme.muted; buttonText: Theme.muted }
    }
}
