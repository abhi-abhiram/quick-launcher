import AppKit
import QuickLauncherKit
import SwiftUI

/// Draws the launcher: the mode chip and search field, the error line and
/// the rows. Everything it shows comes from the core's state.
struct LauncherView: View {
    @ObservedObject var controller: LauncherController

    var body: some View {
        let launcher = controller.launcher
        VStack(alignment: .leading, spacing: Layout.spacing) {
            HStack(spacing: 10) {
                if let mode = launcher.mode {
                    ModeChip(mode: mode)
                }
                SearchField(
                    text: launcher.text, placeholder: launcher.placeholder,
                    focusToken: controller.focusToken, onChange: controller.setText)
            }
            .frame(height: Layout.header)
            if let error = launcher.error {
                Text(error)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(height: Layout.errorHeight)
            }
            ResultList(controller: controller)
                .frame(height: Layout.listHeight(rows: launcher.items.count))
        }
        .padding(Layout.padding)
        .frame(width: Layout.width, alignment: .top)
        .background(
            VisualEffect()
                .clipShape(RoundedRectangle(cornerRadius: Layout.cornerRadius, style: .continuous))
        )
        .overlay(
            RoundedRectangle(cornerRadius: Layout.cornerRadius, style: .continuous)
                .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
        )
        .frame(maxHeight: .infinity, alignment: .top)
    }
}

/// The active mode, before the text.
struct ModeChip: View {
    let mode: Mode

    var body: some View {
        HStack(spacing: 6) {
            if let glyph = mode.icon?.glyph, let font = Glyphs.font(for: glyph, size: 14) {
                Text(glyph).font(font)
            }
            Text(mode.name).font(.system(size: 14, weight: .medium))
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(Capsule().fill(Color.accentColor.opacity(0.22)))
    }
}

struct ResultList: View {
    @ObservedObject var controller: LauncherController

    var body: some View {
        let launcher = controller.launcher
        ScrollViewReader { proxy in
            ScrollView(.vertical) {
                LazyVStack(spacing: Layout.rowSpacing) {
                    ForEach(Array(launcher.items.enumerated()), id: \.element.ref) { index, item in
                        Row(item: item, selected: index == launcher.selectedIndex)
                            .id(item.ref)
                            .contentShape(Rectangle())
                            .onTapGesture { controller.activate(index, action: 0) }
                            .onContinuousHover { phase in
                                if case .active = phase { controller.hover(index) }
                            }
                            // Right-click lists every action, the default first.
                            .contextMenu {
                                ForEach(Array(item.actions.enumerated()), id: \.offset) { actionIndex, action in
                                    Button(action.title) { controller.activate(index, action: actionIndex) }
                                        .disabled(actionIndex == 0 && item.disabled)
                                }
                            }
                    }
                }
            }
            .scrollIndicators(.never)
            .onChange(of: launcher.selectedIndex) { index in
                if launcher.items.indices.contains(index) {
                    proxy.scrollTo(launcher.items[index].ref)
                }
            }
            .overlay {
                if launcher.items.isEmpty {
                    Text(launcher.emptyMessage)
                        .font(.system(size: 16))
                        .foregroundStyle(.secondary)
                }
            }
        }
    }
}

struct Row: View {
    let item: Item
    let selected: Bool

    var body: some View {
        HStack(spacing: 10) {
            IconView(icon: item.icon)
                .frame(width: 28, height: 28)
            VStack(alignment: .leading, spacing: 2) {
                Text(item.label)
                    .font(.system(size: 15, weight: .medium))
                    .lineLimit(1)
                if let detail = item.detail, !detail.isEmpty {
                    Text(detail)
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            .opacity(item.disabled ? 0.45 : 1)
            Spacer(minLength: 8)
            if let badge = item.badge, !badge.isEmpty {
                Text(badge)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            if selected {
                let hints = Launcher.actionHints(item)
                if !hints.isEmpty {
                    Text(hints)
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
        }
        .padding(.horizontal, 10)
        .frame(height: Layout.rowHeight)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(selected ? Color.accentColor.opacity(0.28) : Color.clear)
        )
    }
}

struct IconView: View {
    let icon: Icon?

    var body: some View {
        if let image = Icons.image(for: icon) {
            Image(nsImage: image)
                .resizable()
                .aspectRatio(contentMode: .fit)
        } else if let glyph = icon?.glyph, let font = Glyphs.font(for: glyph, size: 20) {
            Text(glyph).font(font)
        } else {
            Color.clear
        }
    }
}

/// Images for `path` and `name` icons: an app bundle or folder gets its
/// Finder icon, an image file is loaded, and a name is tried as an SF Symbol.
enum Icons {
    private static let cache = NSCache<NSString, NSImage>()

    static func image(for icon: Icon?) -> NSImage? {
        if let path = icon?.path {
            let key = "path:\(path)" as NSString
            if let image = cache.object(forKey: key) { return image }
            var isFolder: ObjCBool = false
            guard FileManager.default.fileExists(atPath: path, isDirectory: &isFolder) else { return nil }
            let image = isFolder.boolValue
                ? NSWorkspace.shared.icon(forFile: path)
                : NSImage(contentsOfFile: path) ?? NSWorkspace.shared.icon(forFile: path)
            cache.setObject(image, forKey: key)
            return image
        }
        if let name = icon?.name {
            return NSImage(systemSymbolName: name, accessibilityDescription: nil)
        }
        return nil
    }
}

/// Glyph icons are Nerd Font code points in the bundled plugins. macOS has
/// no such font, so they draw with an installed Nerd Font, or not at all
/// rather than as a box.
enum Glyphs {
    private static let nerdFont: String? = NSFontManager.shared.availableFontFamilies
        .first { $0.contains("Nerd Font") }

    static func font(for glyph: String, size: CGFloat) -> Font? {
        let privateUse = glyph.unicodeScalars.contains { scalar in
            (0xE000...0xF8FF).contains(scalar.value) || (0xF0000...0x10FFFD).contains(scalar.value)
        }
        guard privateUse else { return .system(size: size) }
        return nerdFont.map { Font.custom($0, size: size) }
    }
}

/// The panel's translucent background.
struct VisualEffect: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .popover
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {}
}

/// The search field. Typing reports the new text; text the core sets
/// (entering a mode, say) replaces the field's without reporting it back.
struct SearchField: NSViewRepresentable {
    var text: String
    var placeholder: String
    var focusToken: Int
    var onChange: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSTextField {
        let field = NSTextField()
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: 22)
        field.usesSingleLineMode = true
        field.lineBreakMode = .byTruncatingHead
        field.cell?.isScrollable = true
        field.delegate = context.coordinator
        return field
    }

    func updateNSView(_ field: NSTextField, context: Context) {
        context.coordinator.parent = self
        field.placeholderString = placeholder
        if field.stringValue != text {
            field.stringValue = text
            field.currentEditor()?.selectedRange = NSRange(location: (text as NSString).length, length: 0)
        }
        if context.coordinator.focusToken != focusToken {
            context.coordinator.focusToken = focusToken
            DispatchQueue.main.async {
                field.window?.makeFirstResponder(field)
                field.currentEditor()?.selectedRange = NSRange(location: (field.stringValue as NSString).length, length: 0)
            }
        }
    }

    final class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: SearchField
        var focusToken = -1

        init(_ parent: SearchField) {
            self.parent = parent
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSTextField else { return }
            parent.onChange(field.stringValue)
        }
    }
}
