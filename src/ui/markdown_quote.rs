use super::{file, origin, *};
use origin::Target;

extern "C" {
    fn gtk_combo_box_text_new() -> *mut GtkWidget;
    fn gtk_combo_box_text_append_text(combo: *mut GtkWidget, text: *const i8);
    fn gtk_combo_box_text_remove_all(combo: *mut GtkWidget);
    fn gtk_combo_box_get_active(combo: *mut GtkWidget) -> c_int;
    fn gtk_combo_box_set_active(combo: *mut GtkWidget, index: c_int);
}

pub(super) struct Picker {
    combo: *mut GtkWidget,
    targets: Vec<Target>,
}

impl Picker {
    pub fn mount(root: *mut GtkWidget) -> Self {
        unsafe {
            let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
            let combo = gtk_combo_box_text_new();
            gtk_box_pack_start(row, combo, 1, 1, 0);
            button(row, text::MD_QUOTE, insert);
            gtk_box_pack_start(root, row, 0, 0, 0);
            Self {
                combo,
                targets: Vec::new(),
            }
        }
    }

    fn selected(&self) -> Option<&Target> {
        let index = unsafe { gtk_combo_box_get_active(self.combo) };
        self.targets.get(usize::try_from(index - 1).ok()?)
    }

    pub fn reset(&mut self, origin: Option<&Origin>) {
        let targets = origin::targets();
        let index = targets.iter().position(|t| Some(&t.origin) == origin);
        self.populate(targets, index);
    }

    pub fn sync(&mut self) {
        let targets = origin::targets();
        if targets != self.targets {
            let selected = self
                .selected()
                .and_then(|old| targets.iter().position(|t| t == old));
            self.populate(targets, selected);
        }
    }

    fn populate(&mut self, targets: Vec<Target>, index: Option<usize>) {
        unsafe {
            gtk_combo_box_text_remove_all(self.combo);
            gtk_combo_box_text_append_text(self.combo, text::MD_TARGET.as_ptr());
            for t in &targets {
                let label = CString::new(t.label.replace('\0', "")).unwrap();
                gtk_combo_box_text_append_text(self.combo, label.as_ptr());
            }
            gtk_combo_box_set_active(self.combo, index.map_or(0, |i| (i + 1) as i32));
        }
        self.targets = targets;
    }
}

pub(super) fn format_quote(location: &str, source: &str, a: i32, b: i32) -> Option<String> {
    let start = file::byte_at(source, a);
    let end = file::byte_at(source, b);
    let selection = source.get(start..end)?.replace("\r\n", "\n");
    if selection.is_empty()
        || location.chars().any(char::is_control)
        || selection
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return None;
    }
    let first = 1 + source[..start].bytes().filter(|&c| c == b'\n').count();
    let last = first
        + selection
            .strip_suffix('\n')
            .unwrap_or(&selection)
            .bytes()
            .filter(|&c| c == b'\n')
            .count();
    let run = selection
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(3.max(run + 1));
    let separator = if selection.ends_with('\n') { "" } else { "\n" };
    Some(format!(
        "{location}#L{first}-L{last}\n{fence}\n{selection}{separator}{fence}"
    ))
}

extern "C" fn insert(_: *mut GtkWidget, _: *mut c_void) {
    let draft = READER.with_borrow(|slot| {
        let r = slot.as_ref()?;
        let target = r.picker.selected().cloned();
        let name = r.path.display().to_string();
        let location = r
            .remote
            .as_ref()
            .map_or_else(|| name.clone(), |d| format!("{}:{name}", d.host));
        let (a, b) = r.buffer.bounds();
        Some((target, format_quote(&location, &r.source, a, b)))
    });
    let Some((target, quote)) = draft else { return };
    let Some(quote) = quote else {
        return app_state::set_footer_message(text::MD_SELECT.as_str());
    };
    let Some(target) = target else {
        return app_state::set_footer_message(text::MD_TARGET.as_str());
    };
    if !origin::is_current(&target) {
        return app_state::set_footer_message(text::MD_STALE.as_str());
    }
    crate::session_ops::select_session(target.sid);
    app_state::record_pane_focus(target.sid, target.vte);
    unsafe { gtk_widget_grab_focus(target.vte) };
    if let Some((connection, address)) = target.delivery {
        std::thread::spawn(move || {
            if let Err(err) = connection.paste(&address, &quote) {
                unsafe {
                    g_idle_add(
                        report_error,
                        Box::into_raw(Box::new(err.to_string())).cast(),
                    );
                }
            }
        });
        return;
    }
    let text = CString::new(quote).unwrap();
    unsafe {
        vte_terminal_paste_text(target.vte, text.as_ptr());
    }
}

extern "C" fn report_error(raw: *mut c_void) -> c_int {
    let message = unsafe { Box::from_raw(raw.cast::<String>()) };
    app_state::set_footer_message(&message);
    0
}
