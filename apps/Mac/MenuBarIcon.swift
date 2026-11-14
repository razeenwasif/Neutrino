import AppKit

/// The menu bar glyph: the app icon reduced to strokes (design/menubar.svg).
/// Drawn as a template image so macOS tints it for the menu bar's appearance.
@MainActor
enum MenuBarIcon {
    enum State { case live, offline }

    private static var cache: [State: NSImage] = [:]

    static func image(_ state: State) -> NSImage {
        if let image = cache[state] { return image }
        let image = NSImage(size: NSSize(width: 18, height: 18), flipped: true) { _ in
            draw(state)
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = state == .live ? "Neutrino" : "Neutrino, neutrinod not running"
        cache[state] = image
        return image
    }

    nonisolated private static func draw(_ state: State) {
        guard let cg = NSGraphicsContext.current?.cgContext else { return }
        cg.translateBy(x: 0.6, y: 0.6)
        // Offline: everything dimmer and the particle hollow.
        let strong: CGFloat = state == .live ? 1 : 0.6
        let faint: CGFloat = state == .live ? 0.45 : 0.3
        let ink = { (alpha: CGFloat) in NSColor.black.withAlphaComponent(alpha) }

        let fork = CGPoint(x: 5.9, y: 12.1)
        let merge = CGPoint(x: 12.1, y: 5.9)
        let particle = CGPoint(x: 6.64, y: 6.64)

        func stroke(_ path: NSBezierPath, _ width: CGFloat, _ alpha: CGFloat) {
            path.lineWidth = width
            path.lineCapStyle = .round
            ink(alpha).setStroke()
            path.stroke()
        }

        // Main track.
        let track = NSBezierPath()
        track.move(to: CGPoint(x: 3, y: 15))
        track.line(to: CGPoint(x: 15, y: 3))
        stroke(track, 1.5, faint)

        // The branch the particle hasn't reached yet, then its trail.
        let ahead = NSBezierPath()
        ahead.move(to: particle)
        ahead.curve(to: merge, controlPoint1: CGPoint(x: 7.985, y: 5.2975), controlPoint2: CGPoint(x: 10.115, y: 4.74))
        stroke(ahead, 1.5, faint)

        let trail = NSBezierPath()
        trail.move(to: fork)
        trail.curve(to: particle, controlPoint1: CGPoint(x: 4.74, y: 10.115), controlPoint2: CGPoint(x: 5.2975, y: 7.985))
        stroke(trail, 1.6, strong)

        // Fork and merge commits: rings punched out of the tracks.
        for point in [fork, merge] {
            let hole = NSBezierPath(ovalIn: CGRect(x: point.x - 1.6, y: point.y - 1.6, width: 3.2, height: 3.2))
            NSGraphicsContext.current?.compositingOperation = .clear
            hole.fill()
            NSGraphicsContext.current?.compositingOperation = .sourceOver
            stroke(NSBezierPath(ovalIn: CGRect(x: point.x - 1.35, y: point.y - 1.35, width: 2.7, height: 2.7)), 1.1, strong)
        }

        // Particle.
        if state == .live {
            ink(1).setFill()
            NSBezierPath(ovalIn: CGRect(x: particle.x - 1.95, y: particle.y - 1.95, width: 3.9, height: 3.9)).fill()
        } else {
            let hole = NSBezierPath(ovalIn: CGRect(x: particle.x - 1.95, y: particle.y - 1.95, width: 3.9, height: 3.9))
            NSGraphicsContext.current?.compositingOperation = .clear
            hole.fill()
            NSGraphicsContext.current?.compositingOperation = .sourceOver
            stroke(NSBezierPath(ovalIn: CGRect(x: particle.x - 1.45, y: particle.y - 1.45, width: 2.9, height: 2.9)), 1.1, strong)
        }
    }
}
