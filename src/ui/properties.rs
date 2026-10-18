//! Dialog for editing an object's headers, custom metadata and tags.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use adw::prelude::*;

use crate::s3::{ObjectDetails, ObjectEdit};

type Pairs = Rc<RefCell<Vec<(gtk::Entry, gtk::Entry, gtk::Box)>>>;

/// A group of editable name/value rows with an Add button.
struct PairsEditor {
    group: adw::PreferencesGroup,
    /// Holds the rows, spaced apart (a preferences group stacks plain widgets with no gap).
    rows: gtk::Box,
    pairs: Pairs,
}

impl PairsEditor {
    fn new(title: &str, description: &str, initial: &BTreeMap<String, String>) -> Self {
        let group = adw::PreferencesGroup::builder()
            .title(title)
            .description(description)
            .build();
        let pairs = Pairs::default();
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        group.set_header_suffix(Some(&add));
        let rows = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        group.add(&rows);
        let editor = Self { group, rows, pairs };
        for (k, v) in initial {
            editor.add_row(k, v);
        }
        add.connect_clicked({
            let rows = editor.rows.clone();
            let pairs = editor.pairs.clone();
            move |_| {
                let key = Self::push_row(&rows, &pairs, "", "");
                key.grab_focus();
            }
        });
        editor
    }

    fn add_row(&self, key: &str, value: &str) {
        Self::push_row(&self.rows, &self.pairs, key, value);
    }

    fn push_row(rows: &gtk::Box, pairs: &Pairs, key: &str, value: &str) -> gtk::Entry {
        let key_entry = gtk::Entry::builder()
            .text(key)
            .placeholder_text("Name")
            .hexpand(true)
            .build();
        let value_entry = gtk::Entry::builder()
            .text(value)
            .placeholder_text("Value")
            .hexpand(true)
            .build();
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Remove")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let row = gtk::Box::builder().spacing(6).build();
        row.append(&key_entry);
        row.append(&value_entry);
        row.append(&remove);
        rows.append(&row);
        pairs
            .borrow_mut()
            .push((key_entry.clone(), value_entry, row.clone()));
        remove.connect_clicked({
            let rows = rows.clone();
            let pairs = pairs.clone();
            move |_| {
                rows.remove(&row);
                pairs.borrow_mut().retain(|(_, _, r)| r != &row);
            }
        });
        key_entry
    }

    /// The non-empty rows. A later row replaces an earlier one with the same name.
    fn values(&self) -> BTreeMap<String, String> {
        self.pairs
            .borrow()
            .iter()
            .map(|(k, v, _)| (k.text().trim().to_string(), v.text().to_string()))
            .filter(|(k, v)| !k.is_empty() || !v.is_empty())
            .collect()
    }
}

/// Opens the editor for `details`. `on_save` receives the validated edit.
pub fn edit_properties(
    parent: &impl IsA<gtk::Widget>,
    details: &ObjectDetails,
    on_save: impl Fn(ObjectEdit) + 'static,
) {
    let current = ObjectEdit::from_details(details);
    let name = super::details::base_name(&details.key);

    let headers = adw::PreferencesGroup::builder()
        .title("Headers")
        .description("Sent back whenever the object is downloaded. Empty fields are removed")
        .build();
    let row = |title: &str, value: &str| {
        let r = adw::EntryRow::builder().title(title).text(value).build();
        headers.add(&r);
        r
    };
    let content_type = row("Content-Type", &current.content_type);
    let cache_control = row("Cache-Control", &current.cache_control);
    let content_encoding = row("Content-Encoding", &current.content_encoding);
    let content_disposition = row("Content-Disposition", &current.content_disposition);
    let content_language = row("Content-Language", &current.content_language);

    let metadata = PairsEditor::new(
        "Custom Metadata",
        "Stored with the object as x-amz-meta-* headers. Names are lowercased by S3",
        &current.metadata,
    );
    let tags = PairsEditor::new(
        "Tags",
        "Up to 10 key/value labels, used by lifecycle rules, billing and access policies",
        &current.tags,
    );
    tags.group.set_visible(details.tags.is_some());

    let note = gtk::Label::builder()
        .label(
            "Changing headers or metadata rewrites the object (it gets a new modification date, and a \
             new version in versioned buckets). Changing only tags does not.",
        )
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&headers);
    content.append(&metadata.group);
    content.append(&tags.group);
    content.append(&note);

    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::builder()
        .label("Save")
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::builder()
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .title_widget(&adw::WindowTitle::new("Edit Properties", name))
        .build();
    header.pack_start(&cancel);
    header.pack_end(&save);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&adw::Clamp::builder().child(&content).build())
            .build(),
    ));

    let dialog = adw::Dialog::builder()
        .title("Edit Properties")
        .content_width(560)
        .content_height(680)
        .child(&view)
        .build();
    cancel.connect_clicked({
        let d = dialog.clone();
        move |_| {
            d.close();
        }
    });
    let keep_tags = details.tags.is_none();
    save.connect_clicked({
        let d = dialog.clone();
        let original = current.tags.clone();
        move |_| {
            let edit = ObjectEdit {
                content_type: content_type.text().trim().to_string(),
                cache_control: cache_control.text().trim().to_string(),
                content_encoding: content_encoding.text().trim().to_string(),
                content_disposition: content_disposition.text().trim().to_string(),
                content_language: content_language.text().trim().to_string(),
                metadata: metadata.values(),
                tags: if keep_tags {
                    original.clone()
                } else {
                    tags.values()
                },
            };
            match edit.validate() {
                Ok(()) => {
                    d.close();
                    on_save(edit);
                }
                Err(message) => super::dialogs::error(&d, "Can't Save", &message),
            }
        }
    });
    dialog.present(Some(parent));
}
