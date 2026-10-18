//! Small modal dialogs built on `AdwAlertDialog`.

use std::time::Duration;

use adw::prelude::*;

/// Asks for one line of text. `validate` returns an error hint for invalid input.
#[allow(clippy::too_many_arguments)]
pub fn ask_text(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    field: &str,
    initial: &str,
    select: Option<(i32, i32)>,
    accept: &str,
    validate: impl Fn(&str) -> Result<(), String> + 'static,
    on_accept: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), (!body.is_empty()).then_some(body));
    dialog.add_responses(&[("cancel", "Cancel"), ("accept", accept)]);
    dialog.set_response_appearance("accept", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("accept"));
    dialog.set_close_response("cancel");

    let entry = adw::EntryRow::builder()
        .title(field)
        .text(initial)
        .activates_default(true)
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    list.append(&entry);
    let hint = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["error", "caption"])
        .visible(false)
        .build();
    let extra = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    extra.append(&list);
    extra.append(&hint);
    dialog.set_extra_child(Some(&extra));

    // "accept" is only enabled for valid input. The error hint only appears once the user has
    // edited the field, so an untouched dialog never opens with an error.
    dialog.set_response_enabled("accept", validate(initial).is_ok());
    let d = dialog.clone();
    entry.connect_changed(move |e| {
        let text = e.text();
        let result = validate(&text);
        d.set_response_enabled("accept", result.is_ok());
        match result {
            Err(msg) if !text.is_empty() => {
                hint.set_label(&msg);
                hint.set_visible(true);
                e.add_css_class("error");
            }
            _ => {
                hint.set_visible(false);
                e.remove_css_class("error");
            }
        }
    });
    dialog.connect_response(Some("accept"), {
        let entry = entry.clone();
        move |_, _| on_accept(entry.text().to_string())
    });
    dialog.present(Some(parent));
    entry.grab_focus();
    if let Some((start, end)) = select {
        entry.select_region(start, end);
    }
}

/// Asks for confirmation; `destructive` paints the accept button red.
pub fn confirm(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    accept: &str,
    destructive: bool,
    on_accept: impl Fn() + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_responses(&[("cancel", "Cancel"), ("accept", accept)]);
    dialog.set_response_appearance(
        "accept",
        if destructive {
            adw::ResponseAppearance::Destructive
        } else {
            adw::ResponseAppearance::Suggested
        },
    );
    // Destructive actions must be an explicit click, never a stray Enter.
    dialog.set_default_response(Some(if destructive { "cancel" } else { "accept" }));
    dialog.set_close_response("cancel");
    dialog.connect_response(Some("accept"), move |_, _| on_accept());
    dialog.present(Some(parent));
}

/// Confirms deleting a bucket, optionally emptying it first.
pub fn delete_bucket(
    parent: &impl IsA<gtk::Widget>,
    name: &str,
    on_accept: impl Fn(bool) + 'static,
) {
    let dialog = adw::AlertDialog::new(
        Some("Delete Bucket?"),
        Some(&format!("The bucket “{name}” will be permanently deleted.")),
    );
    dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let force = adw::SwitchRow::builder()
        .title("Also Delete All Objects")
        .subtitle("Otherwise only an empty bucket can be deleted")
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    list.append(&force);
    dialog.set_extra_child(Some(&list));
    dialog.connect_response(Some("delete"), move |_, _| on_accept(force.is_active()));
    dialog.present(Some(parent));
}

/// Link lifetimes: unit name, singular, seconds per unit, largest amount (S3 signatures last at most 7 days).
const LINK_UNITS: [(&str, &str, u64, u32); 3] = [
    ("Minutes", "minute", 60, 7 * 24 * 60),
    ("Hours", "hour", 3600, 7 * 24),
    ("Days", "day", 86_400, 7),
];
/// Index of "Days" in [`LINK_UNITS`], the default unit.
const DEFAULT_LINK_UNIT: usize = 2;

fn link_duration(amount: u32, unit: usize) -> Duration {
    Duration::from_secs(u64::from(amount) * LINK_UNITS[unit].2)
}

fn link_label(amount: u32, unit: usize) -> String {
    let name = LINK_UNITS[unit].1;
    if amount == 1 {
        format!("1 {name}")
    } else {
        format!("{amount} {name}s")
    }
}

/// Lets the user pick how long a presigned link stays valid, from one minute up to the 7 days S3 allows.
pub fn share_link(
    parent: &impl IsA<gtk::Widget>,
    name: &str,
    on_accept: impl Fn(Duration, String) + 'static,
) {
    let dialog = adw::AlertDialog::new(
        Some("Share Link"),
        Some(&format!(
            "Anyone with the link can download “{name}” until it expires. The link is copied to the clipboard."
        )),
    );
    dialog.add_responses(&[("cancel", "Cancel"), ("copy", "Copy Link")]);
    dialog.set_response_appearance("copy", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("copy"));
    dialog.set_close_response("cancel");

    let amount = adw::SpinRow::with_range(1.0, f64::from(LINK_UNITS[DEFAULT_LINK_UNIT].3), 1.0);
    amount.set_title("Valid For");
    let units = gtk::StringList::new(&LINK_UNITS.map(|u| u.0));
    let unit = adw::ComboRow::builder()
        .title("Unit")
        .model(&units)
        .selected(DEFAULT_LINK_UNIT as u32)
        .build();
    // Each unit has its own maximum, so the total never goes past 7 days.
    unit.connect_selected_notify({
        let amount = amount.clone();
        move |u| {
            let max = LINK_UNITS[u.selected() as usize].3;
            amount.adjustment().set_upper(f64::from(max));
            amount.set_value(amount.value().min(f64::from(max)));
        }
    });
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    list.append(&amount);
    list.append(&unit);
    dialog.set_extra_child(Some(&list));
    dialog.connect_response(Some("copy"), move |_, _| {
        let (n, u) = (amount.value() as u32, unit.selected() as usize);
        on_accept(link_duration(n, u), link_label(n, u));
    });
    dialog.present(Some(parent));
}

/// Reports a failure. Long messages (e.g. per-object delete failures) go into a scrollable area.
pub fn error(parent: &impl IsA<gtk::Widget>, heading: &str, message: &str) {
    let mut lines = message.lines();
    let first = lines.next().unwrap_or_default();
    let rest: Vec<&str> = lines.collect();
    let dialog = adw::AlertDialog::new(Some(heading), Some(first));
    if !rest.is_empty() {
        let details: Vec<&str> = rest
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let label = gtk::Label::builder()
            .label(details.join("\n"))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            // Focusing a selectable label selects all of it; mouse selection works without focus.
            .focusable(false)
            .css_classes(["caption"])
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&label)
            .propagate_natural_height(true)
            .max_content_height(260)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        dialog.set_extra_child(Some(&scrolled));
    }
    dialog.add_responses(&[("copy", "Copy Details"), ("close", "OK")]);
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    let text = format!("{heading}\n{message}");
    let clipboard = parent.as_ref().clipboard();
    dialog.connect_response(Some("copy"), move |_, _| clipboard.set_text(&text));
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_durations() {
        assert_eq!(link_duration(30, 0), Duration::from_secs(30 * 60));
        assert_eq!(link_duration(12, 1), Duration::from_secs(12 * 3600));
        assert_eq!(link_duration(7, 2), Duration::from_secs(7 * 86_400));
    }

    #[test]
    fn link_labels() {
        assert_eq!(link_label(1, 1), "1 hour");
        assert_eq!(link_label(3, 2), "3 days");
        assert_eq!(link_label(45, 0), "45 minutes");
    }

    #[test]
    fn every_unit_tops_out_at_seven_days() {
        for (i, u) in LINK_UNITS.iter().enumerate() {
            assert_eq!(link_duration(u.3, i), Duration::from_secs(7 * 86_400));
        }
    }
}
