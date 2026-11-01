import AppKit
import QuickLauncherKit
import SwiftUI

/// Draws the launcher: the mode chip and search field, the error line, the
/// rows and the footer with the selected row's keys. Everything it shows
/// comes from the core's state; the active mode's ``ModeStyle`` tints it.
struct LauncherView: View {
    @ObservedObject var controller: LauncherController

    var body: some View {
        let launcher = controller.launcher
        let style = ModeStyle.of(launcher.mode)
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                if let mode = launcher.mode {
                    ModeChip(mode: mode, style: style)
                } else {
                    Image(systemName: "magnifyingglass")
                        .font(.system(size: 18, weight: .medium))
                        .foregroundStyle(.secondary)
                }
                SearchField(
                    text: launcher.text, placeholder: launcher.placeholder,
                    monospaced: style.monospaced, caret: style.nsColor,
                    focusToken: controller.focusToken, onChange: controller.setText)
            }
            .padding(.horizontal, 16)
            .frame(height: Layout.header)
            Hairline()
            if let error = launcher.error {
                Text(error)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .padding(.horizontal, 16)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .frame(height: Layout.errorHeight)
            }
            ResultList(controller: controller, style: style)
                .frame(height: Layout.listHeight(rows: launcher.items.count))
                .padding(Layout.listPadding)
            Footer(launcher: launcher, style: style)
        }
        .frame(width: Layout.width, alignment: .top)
        .background(VisualEffect())
        .clipShape(RoundedRectangle(cornerRadius: Layout.cornerRadius, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: Layout.cornerRadius, style: .continuous)
                .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
        )
        .frame(maxHeight: .infinity, alignment: .top)
    }
}

/// The 1pt line under the header and over the footer.
struct Hairline: View {
    var body: some View {
        Rectangle().fill(Color.primary.opacity(0.08)).frame(height: 1)
    }
}

extension ModeStyle {
    var nsColor: NSColor {
        guard let tint else { return .controlAccentColor }
        return NSColor(
            srgbRed: CGFloat((tint >> 16) & 0xFF) / 255, green: CGFloat((tint >> 8) & 0xFF) / 255,
            blue: CGFloat(tint & 0xFF) / 255, alpha: 1)
    }

    var color: Color { Color(nsColor: nsColor) }

    /// The selected row's fill: the mode's tint in a mode, a neutral one in
    /// the plain search, where the rows are other apps' icons.
    var selectionFill: Color {
        tint == nil ? Color.primary.opacity(0.09) : color.opacity(0.2)
    }
}

/// The active mode, before the text, in the mode's tint.
struct ModeChip: View {
    let mode: Mode
    let style: ModeStyle

    var body: some View {
        HStack(spacing: 6) {
            SymbolOrGlyph(icon: mode.icon, style: style, size: 13)
            Text(mode.name).font(.system(size: 14, weight: .semibold))
        }
        .foregroundStyle(style.color)
        .padding(.horizontal, 10)
        .frame(height: 28)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(style.color.opacity(0.2)))
    }
}

struct ResultList: View {
    @ObservedObject var controller: LauncherController
    let style: ModeStyle

    var body: some View {
        let launcher = controller.launcher
        ScrollViewReader { proxy in
            ScrollView(.vertical) {
                LazyVStack(spacing: Layout.rowSpacing) {
                    ForEach(Array(launcher.items.enumerated()), id: \.element.ref) { index, item in
                        Row(item: item, selected: index == launcher.selectedIndex, style: style)
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
    let style: ModeStyle

    var body: some View {
        HStack(spacing: 12) {
            RowIcon(icon: item.icon, style: style)
                .frame(width: 32, height: 32)
            VStack(alignment: .leading, spacing: 1) {
                Text(item.label)
                    .font(style.monospaced
                        ? .system(size: 13, weight: .regular, design: .monospaced)
                        : .system(size: 14, weight: .medium))
                    .lineLimit(1)
                if let detail = item.detail, !detail.isEmpty {
                    DetailText(detail: detail)
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
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(style.color)
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2)
                    .background(RoundedRectangle(cornerRadius: 5, style: .continuous).fill(style.color.opacity(0.16)))
            }
        }
        .padding(.horizontal, 10)
        .frame(height: Layout.rowHeight)
        .background(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(selected ? style.selectionFill : Color.clear)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .strokeBorder(selected ? style.color.opacity(0.45) : Color.clear, lineWidth: 1)
        )
    }
}

/// A detail that's a path reads as breadcrumbs, its last folder in bold:
/// `~ › Projects › quick-launcher`. Any other detail is drawn as it is.
struct DetailText: View {
    let detail: String

    var body: some View {
        if let parts = Breadcrumbs.split(detail), let last = parts.last, parts.count > 1 {
            Text(parts.dropLast().joined(separator: " › ") + " › ")
                + Text(last).fontWeight(.semibold).foregroundColor(Color.primary.opacity(0.85))
        } else {
            Text(detail)
        }
    }
}

/// A row's icon. A file's icon (an app, a folder, an image) draws as it is;
/// anything else sits on a tile in the mode's tint: an SF Symbol it names,
/// its glyph when a font has it, or else the mode's own symbol.
struct RowIcon: View {
    let icon: Icon?
    let style: ModeStyle

    var body: some View {
        if icon?.path != nil, let image = Icons.image(for: icon) {
            Image(nsImage: image)
                .resizable()
                .aspectRatio(contentMode: .fit)
        } else {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(style.color.opacity(0.22))
                .overlay(SymbolOrGlyph(icon: icon, style: style, size: 16).foregroundStyle(style.color))
        }
    }
}

/// An icon's SF Symbol or glyph, or the mode's symbol when neither draws.
struct SymbolOrGlyph: View {
    let icon: Icon?
    let style: ModeStyle
    let size: CGFloat

    var body: some View {
        if let name = icon?.name, NSImage(systemSymbolName: name, accessibilityDescription: nil) != nil {
            Image(systemName: name).font(.system(size: size, weight: .medium))
        } else if let glyph = icon?.glyph, let font = Glyphs.font(for: glyph, size: size + 2) {
            Text(glyph).font(font)
        } else {
            Image(systemName: style.symbol).font(.system(size: size, weight: .medium))
        }
    }
}

/// The bottom bar: the mode on the left, and on the right the selected
/// row's actions with their keys, then what Escape does.
struct Footer: View {
    let launcher: Launcher
    let style: ModeStyle

    var body: some View {
        HStack(spacing: 14) {
            HStack(spacing: 7) {
                if launcher.mode == nil {
                    Image(systemName: style.symbol).font(.system(size: 11, weight: .medium))
                } else {
                    Circle().fill(style.color).frame(width: 8, height: 8)
                }
                Text(launcher.mode?.name ?? "Apps").lineLimit(1)
            }
            Spacer(minLength: 8)
            if let item = launcher.selected {
                ForEach(Array(Launcher.keyHints(item).enumerated()), id: \.offset) { _, hint in
                    KeyHintView(hint: hint)
                }
            }
            KeyHintView(hint: KeyHint(keys: ["⎋"], title: launcher.escapeTitle))
        }
        .font(.system(size: 12))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 14)
        .frame(maxWidth: .infinity)
        .frame(height: Layout.footer)
        .background(Color.primary.opacity(0.04))
        .overlay(alignment: .top) { Hairline() }
    }
}

struct KeyHintView: View {
    let hint: KeyHint

    var body: some View {
        HStack(spacing: 6) {
            Text(hint.title).lineLimit(1)
            HStack(spacing: 3) {
                ForEach(hint.keys, id: \.self) { key in
                    Text(key)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundColor(Color.primary.opacity(0.85))
                        .padding(.horizontal, 5)
                        .frame(minWidth: 18, minHeight: 18)
                        .background(RoundedRectangle(cornerRadius: 5, style: .continuous).fill(Color.primary.opacity(0.1)))
                }
            }
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
/// The caret takes the mode's tint, and a mode of commands types in a
/// monospaced font.
struct SearchField: NSViewRepresentable {
    var text: String
    var placeholder: String
    var monospaced: Bool
    var caret: NSColor
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
        let font: NSFont = monospaced
            ? .monospacedSystemFont(ofSize: 20, weight: .regular)
            : .systemFont(ofSize: 22)
        if field.font != font { field.font = font }
        if field.stringValue != text {
            field.stringValue = text
            field.currentEditor()?.selectedRange = NSRange(location: (text as NSString).length, length: 0)
        }
        (field.currentEditor() as? NSTextView)?.insertionPointColor = caret
        if context.coordinator.focusToken != focusToken {
            context.coordinator.focusToken = focusToken
            DispatchQueue.main.async {
                field.window?.makeFirstResponder(field)
                field.currentEditor()?.selectedRange = NSRange(location: (field.stringValue as NSString).length, length: 0)
                (field.currentEditor() as? NSTextView)?.insertionPointColor = caret
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
