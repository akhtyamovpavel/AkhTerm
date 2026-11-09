//! shadcn/ui Dark Zinc GTK3 CSS stylesheet and VTE-2.91 16-color palette.

use crate::ffi::GdkRGBA;

pub const SHADCN_GTK_CSS: &str = concat!(r#"
window, .main-bg {
    background-color: #09090b;
    color: #fafafa;
}

.topbar {
    background-color: #09090b;
    border-bottom: 1px solid #27272a;
    padding: 5px 10px;
}

.brand-label {
    color: #fafafa;
    font-weight: 600;
    padding: 0 10px 0 4px;
}

.section-header {
    color: #71717a;
    font-size: 0.9em;
    font-weight: 600;
    padding: 8px 6px 3px 6px;
}

.footer-bar {
    background-color: #0c0c0f;
    border-top: 1px solid #27272a;
    padding: 4px 12px;
    color: #a1a1aa;
    font-size: 0.9em;
}

button {
    background-image: none;
    background-color: #121216;
    color: #e4e4e7;
    border: 1px solid #27272a;
    border-radius: 6px;
    padding: 4px 10px;
    box-shadow: none;
    text-shadow: none;
}

button:hover {
    background-color: #1c1c22;
    border-color: #3f3f46;
    color: #ffffff;
}

button.tab-unread {
    border: 1px solid #10b981;
    color: #6ee7b7;
}

button.btn-violet {
    background-color: rgba(139, 92, 246, 0.14);
    border-color: rgba(139, 92, 246, 0.45);
    color: #ddd6fe;
    font-weight: 600;
}

button.btn-violet:hover {
    background-color: rgba(139, 92, 246, 0.26);
    border-color: #8b5cf6;
}

button.btn-mi-a, button.btn-mi-b, button.btn-mi-c {
    font-weight: 600;
}

button.btn-mi-a {
    background-color: rgba(16, 185, 129, 0.14);
    border-color: rgba(16, 185, 129, 0.48);
    color: #6ee7b7;
}

button.btn-mi-b {
    background-color: rgba(245, 158, 11, 0.15);
    border-color: rgba(245, 158, 11, 0.48);
    color: #fcd34d;
}

button.btn-mi-c {
    background-color: rgba(244, 63, 94, 0.15);
    border-color: rgba(244, 63, 94, 0.48);
    color: #fda4af;
}

.ssh-count {
    color: #a1a1aa;
    font-size: 0.9em;
}

button.tmux-jump-btn {
    background-color: #121217;
    border: 1px solid #27272a;
    border-radius: 6px;
    padding: 5px 9px;
    color: #e4e4e7;
}

button.tmux-jump-btn:hover {
    background-color: rgba(245, 158, 11, 0.14);
    border-color: #f59e0b;
    color: #fcd34d;
}

button.tmux-jump-active {
    background-color: rgba(245, 158, 11, 0.14);
    border-color: rgba(245, 158, 11, 0.55);
    color: #fcd34d;
    font-weight: 600;
}

popover, .popover-box {
    background-color: #0c0c0f;
    color: #fafafa;
    border: 1px solid #27272a;
    border-radius: 8px;
    padding: 6px;
}

entry, .search-entry {
    background-image: none;
    background-color: #121217;
    color: #fafafa;
    caret-color: #8b5cf6;
    border: 1px solid #27272a;
    border-radius: 6px;
    padding: 5px 9px;
    box-shadow: none;
}

entry:focus, .search-entry:focus {
    border-color: #8b5cf6;
    background-color: #16161d;
}

scrollbar {
    background-color: #09090b;
    border: none;
}

scrollbar slider {
    background-color: #3f3f46;
    border-radius: 99px;
    min-width: 7px;
    min-height: 30px;
    margin: 2px;
}

scrollbar slider:hover {
    background-color: #8b5cf6;
}

button.btn-icon-sq {
    background-color: #141419;
    border: 1px solid #27272a;
    border-radius: 6px;
    padding: 4px 8px;
    color: #e4e4e7;
    font-weight: 700;
}

button.btn-icon-sq:hover {
    background-color: rgba(139, 92, 246, 0.22);
    border-color: #8b5cf6;
    color: #ffffff;
}

.gpu-pill {
    background-color: #141419;
    border: 1px solid #27272a;
    border-radius: 6px;
    padding: 6px 10px;
    color: #c4b5fd;
    font-size: 0.9em;
}
"#, include_str!("tab_strip.css"), include_str!("sidebar.css"));

pub const TERM_BG: GdkRGBA = GdkRGBA::rgb(9, 9, 11);
pub const TERM_FG: GdkRGBA = GdkRGBA::rgb(250, 250, 250);
pub const TERM_CURSOR: GdkRGBA = GdkRGBA::rgb(139, 92, 246);

pub const TERM_PALETTE: [GdkRGBA; 16] = [
    GdkRGBA::rgb(24, 24, 27),    // 0: Black (Zinc-900)
    GdkRGBA::rgb(244, 63, 94),   // 1: Red (Rose-500)
    GdkRGBA::rgb(16, 185, 129),  // 2: Green (Emerald-500)
    GdkRGBA::rgb(245, 158, 11),  // 3: Yellow (Amber-500)
    GdkRGBA::rgb(59, 130, 246),  // 4: Blue (Blue-500)
    GdkRGBA::rgb(139, 92, 246),  // 5: Magenta (Violet-500)
    GdkRGBA::rgb(6, 182, 212),   // 6: Cyan (Cyan-500)
    GdkRGBA::rgb(228, 228, 231), // 7: White (Zinc-200)
    GdkRGBA::rgb(82, 82, 91),    // 8: Bright Black (Zinc-600)
    GdkRGBA::rgb(251, 113, 133), // 9: Bright Red (Rose-400)
    GdkRGBA::rgb(52, 211, 153),  // 10: Bright Green (Emerald-400)
    GdkRGBA::rgb(251, 191, 36),  // 11: Bright Yellow (Amber-400)
    GdkRGBA::rgb(96, 165, 250),  // 12: Bright Blue (Blue-400)
    GdkRGBA::rgb(167, 139, 250), // 13: Bright Magenta (Violet-400)
    GdkRGBA::rgb(34, 211, 238),  // 14: Bright Cyan (Cyan-400)
    GdkRGBA::rgb(255, 255, 255), // 15: Bright White
];
