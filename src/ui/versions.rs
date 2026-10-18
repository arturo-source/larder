//! Dialog listing the stored versions of an object, to download, restore or permanently delete them.

use std::future::Future;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::details::base_name;
use super::{Win, dialogs};
use crate::runtime::bg;
use crate::s3::{self, Entry, ObjectVersion, human_size};

struct VersionsView {
    dialog: adw::Dialog,
    list: gtk::ListBox,
    stack: gtk::Stack,
    status: gtk::Label,
    entry: Entry,
}

impl Win {
    pub(super) fn show_versions(self: &Rc<Self>) {
        let selected = self.objects.selected();
        let [entry] = selected.as_slice() else { return };
        if entry.is_dir {
            return;
        }
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let status = gtk::Label::builder()
            .wrap(true)
            .css_classes(["dim-label"])
            .margin_top(24)
            .margin_bottom(24)
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(
            &gtk::Spinner::builder()
                .spinning(true)
                .height_request(32)
                .margin_top(24)
                .build(),
            Some("loading"),
        );
        stack.add_named(&list, Some("list"));
        stack.add_named(&status, Some("status"));
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(
            &gtk::Label::builder()
                .label(
                    "Restoring copies an old version on top as the newest one. Deleting a version removes \
                     it for good; deleting a delete marker brings the object back.",
                )
                .wrap(true)
                .xalign(0.0)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
        content.append(&stack);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&adw::Clamp::builder().child(&content).build())
                .build(),
        ));
        let dialog = adw::Dialog::builder()
            .title(format!("Versions of {}", base_name(&entry.key)))
            .content_width(560)
            .content_height(520)
            .child(&view)
            .build();
        let view = Rc::new(VersionsView {
            dialog: dialog.clone(),
            list,
            stack,
            status,
            entry: entry.clone(),
        });
        dialog.present(Some(&self.window));
        self.load_versions(&view);
    }

    fn load_versions(self: &Rc<Self>, view: &Rc<VersionsView>) {
        let Some((client, bucket, _)) = self.location() else {
            return;
        };
        view.stack.set_visible_child_name("loading");
        let (this, view) = (self.clone(), view.clone());
        glib::spawn_future_local(async move {
            let key = view.entry.key.clone();
            let result = bg(async move { s3::list_versions(&client, &bucket, &key).await }).await;
            match result {
                Ok(versions) if versions.is_empty() => {
                    view.status.set_label("This object has no stored versions");
                    view.stack.set_visible_child_name("status");
                }
                Ok(versions) => {
                    this.fill_versions(&view, &versions);
                    view.stack.set_visible_child_name("list");
                }
                Err(e) => {
                    view.status.set_label(&format!("{e:#}"));
                    view.stack.set_visible_child_name("status");
                }
            }
        });
    }

    fn fill_versions(self: &Rc<Self>, view: &Rc<VersionsView>, versions: &[ObjectVersion]) {
        view.list.remove_all();
        for v in versions {
            let when = v.modified.clone().unwrap_or_default();
            let row = adw::ActionRow::builder()
                .title(if v.is_delete_marker {
                    format!("Delete marker · {when}")
                } else {
                    when
                })
                .subtitle(format!(
                    "{}{} · {}",
                    if v.is_delete_marker {
                        String::new()
                    } else {
                        format!("{} · ", human_size(v.size))
                    },
                    if v.is_latest { "current" } else { "older" },
                    v.version_id
                ))
                .subtitle_selectable(true)
                .build();
            let button = |icon: &str, tip: &str| {
                let b = gtk::Button::builder()
                    .icon_name(icon)
                    .tooltip_text(tip)
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .build();
                row.add_suffix(&b);
                b
            };
            if !v.is_delete_marker {
                let download = button("folder-download-symbolic", "Download This Version");
                let (this, view, id) = (self.clone(), view.clone(), v.version_id.clone());
                download.connect_clicked(move |_| this.download_version(&view, &id));
            }
            if !v.is_delete_marker && !v.is_latest {
                let restore = button("edit-undo-symbolic", "Restore This Version");
                let (this, view, id) = (self.clone(), view.clone(), v.version_id.clone());
                restore.connect_clicked(move |_| {
                    let (client, bucket) = match this.location() {
                        Some((c, b, _)) => (c, b),
                        None => return,
                    };
                    let key = view.entry.key.clone();
                    let id = id.clone();
                    this.version_op(&view, "Restore", "Restored the older version", async move {
                        s3::restore_version(&client, &bucket, &key, &id).await
                    });
                });
            }
            let delete = button(
                "user-trash-symbolic",
                if v.is_delete_marker {
                    "Remove Delete Marker"
                } else {
                    "Delete This Version Permanently"
                },
            );
            delete.add_css_class("destructive-action");
            let (this, view_c, id, marker) = (
                self.clone(),
                view.clone(),
                v.version_id.clone(),
                v.is_delete_marker,
            );
            delete.connect_clicked(move |_| {
                let (this2, view2, id2) = (this.clone(), view_c.clone(), id.clone());
                let parent = view_c.dialog.clone();
                dialogs::confirm(
                    &parent,
                    if marker {
                        "Remove Delete Marker?"
                    } else {
                        "Delete Version Permanently?"
                    },
                    if marker {
                        "The object will show up again."
                    } else {
                        "This version can't be recovered."
                    },
                    if marker { "Remove" } else { "Delete" },
                    !marker,
                    move || {
                        let Some((client, bucket, _)) = this2.location() else {
                            return;
                        };
                        let key = view2.entry.key.clone();
                        let id = id2.clone();
                        this2.version_op(&view2, "Delete", "Deleted the version", async move {
                            s3::delete_version(&client, &bucket, &key, &id).await
                        });
                    },
                );
            });
            view.list.append(&row);
        }
    }

    /// Runs a change to the object's versions, then refreshes the dialog, the listing and the details pane.
    fn version_op(
        self: &Rc<Self>,
        view: &Rc<VersionsView>,
        title: &'static str,
        done: &'static str,
        fut: impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) {
        view.stack.set_visible_child_name("loading");
        let (this, view) = (self.clone(), view.clone());
        glib::spawn_future_local(async move {
            match bg(fut).await {
                Ok(()) => this.toast(done),
                Err(e) => this.error(&format!("{title} Failed"), &format!("{e:#}")),
            }
            this.state.borrow_mut().details_key = None;
            this.load_details(view.entry.clone());
            this.reload();
            this.load_versions(&view);
        });
    }

    fn download_version(self: &Rc<Self>, view: &Rc<VersionsView>, version_id: &str) {
        let Some((client, bucket, _)) = self.location() else {
            return;
        };
        let (this, view, id) = (self.clone(), view.clone(), version_id.to_string());
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title("Save Version")
                .initial_name(base_name(&view.entry.key))
                .modal(true)
                .build();
            let Ok(file) = dialog.save_future(Some(&this.window)).await else {
                return;
            };
            let Some(dest) = file.path() else { return };
            let key = view.entry.key.clone();
            match bg(
                async move { s3::download_version(&client, &bucket, &key, Some(&id), &dest).await },
            )
            .await
            {
                Ok(n) => this.toast(&format!("Downloaded, {}", human_size(n as i64))),
                Err(e) => this.error("Download Failed", &format!("{e:#}")),
            }
        });
    }
}
