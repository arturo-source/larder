//! The object table: a sortable, filterable, multi-select column view with a context menu
//! and a drop target for uploads.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib, graphene};

use crate::s3::{Entry, human_size};

pub struct ObjectsView {
    pub root: gtk::Overlay,
    store: gio::ListStore,
    filtered: gtk::FilterListModel,
    filter: gtk::CustomFilter,
    filter_text: Rc<RefCell<String>>,
    pub selection: gtk::MultiSelection,
    pub column_view: gtk::ColumnView,
    stack: gtk::Stack,
    empty_page: adw::StatusPage,
    error_page: adw::StatusPage,
    pub retry_button: gtk::Button,
    drop_target: gtk::DropTarget,
    drop_hint: gtk::Revealer,
    drop_label: gtk::Label,
}

/// Borrows the `Entry` inside a list item.
pub fn entry_of(obj: &glib::Object) -> std::cell::Ref<'_, Entry> {
    obj.downcast_ref::<glib::BoxedAnyObject>()
        .expect("list items are BoxedAnyObject")
        .borrow::<Entry>()
}

fn sorter(cmp: impl Fn(&Entry, &Entry) -> Ordering + 'static) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |a, b| cmp(&entry_of(a), &entry_of(b)).into())
}

/// Symbolic icon for an object, based on its file name.
fn icon_for(e: &Entry) -> gio::Icon {
    if e.is_dir {
        return gio::ThemedIcon::new("folder-symbolic").upcast();
    }
    let (content_type, _) = gio::content_type_guess(Some(e.name.as_str()), None);
    gio::content_type_get_symbolic_icon(&content_type)
}

impl ObjectsView {
    pub fn new(context_menu: &gio::MenuModel) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let filter_text = Rc::new(RefCell::new(String::new()));
        let filter = gtk::CustomFilter::new({
            let text = filter_text.clone();
            move |obj| {
                let needle = text.borrow();
                needle.is_empty() || entry_of(obj).name.to_lowercase().contains(needle.as_str())
            }
        });
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));

        let column_view = gtk::ColumnView::builder().css_classes(["objects"]).build();
        // Folders always come first, then whatever column the user sorts by.
        let sorter_chain = gtk::MultiSorter::new();
        sorter_chain.append(sorter(|a, b| b.is_dir.cmp(&a.is_dir)));
        if let Some(s) = column_view.sorter() {
            sorter_chain.append(s);
        }
        let sorted = gtk::SortListModel::new(Some(filtered.clone()), Some(sorter_chain));
        let selection = gtk::MultiSelection::new(Some(sorted));
        column_view.set_model(Some(&selection));

        let menu = gtk::PopoverMenu::builder()
            .menu_model(context_menu)
            .has_arrow(false)
            .halign(gtk::Align::Start)
            .build();
        menu.set_parent(&column_view);

        let ctx = CellContext {
            selection: selection.clone(),
            menu: menu.clone(),
            column_view: column_view.clone(),
        };

        // Clicking the empty space below the rows clears the selection (and so closes the details pane).
        let blank = gtk::GestureClick::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        blank.connect_pressed({
            let selection = selection.clone();
            let column_view = column_view.downgrade();
            move |_, _, x, y| {
                let Some(column_view) = column_view.upgrade() else {
                    return;
                };
                let mut current = column_view.pick(x, y, gtk::PickFlags::DEFAULT);
                let mut on_list = false;
                while let Some(widget) = current {
                    if widget == *column_view.upcast_ref::<gtk::Widget>() {
                        break;
                    }
                    match widget.css_name().as_str() {
                        "row" | "header" | "scrollbar" => return,
                        "listview" => on_list = true,
                        _ => {}
                    }
                    current = widget.parent();
                }
                if on_list {
                    selection.unselect_all();
                }
            }
        });
        column_view.add_controller(blank);

        let name_col = column(
            &ctx,
            "Name",
            || {
                let bx = gtk::Box::builder()
                    .spacing(10)
                    .hexpand(true)
                    .width_request(220)
                    .build();
                bx.append(&gtk::Image::new());
                bx.append(
                    &gtk::Label::builder()
                        .xalign(0.0)
                        .ellipsize(gtk::pango::EllipsizeMode::Middle)
                        .build(),
                );
                bx.upcast()
            },
            |w, e| {
                let icon = w.first_child().and_downcast::<gtk::Image>().unwrap();
                icon.set_from_gicon(&icon_for(e));
                let label = w.last_child().and_downcast::<gtk::Label>().unwrap();
                label.set_label(e.name.trim_end_matches('/'));
            },
        );
        name_col.set_expand(true);
        name_col.set_resizable(true);
        name_col.set_sorter(Some(&sorter(|a, b| {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        })));

        let size_col = column(
            &ctx,
            "Size",
            || label_cell(1.0, &["numeric"]),
            |w, e| {
                set_label(
                    w,
                    &if e.is_dir {
                        String::new()
                    } else {
                        human_size(e.size)
                    },
                );
            },
        );
        size_col.set_fixed_width(100);
        size_col.set_sorter(Some(&sorter(|a, b| a.size.cmp(&b.size))));

        let modified_col = column(
            &ctx,
            "Modified",
            || label_cell(0.0, &["dim-label", "numeric"]),
            |w, e| {
                // Seconds are noise in a list; the details pane has the full timestamp.
                set_label(
                    w,
                    e.modified
                        .as_deref()
                        .map(|m| m.get(..16).unwrap_or(m))
                        .unwrap_or(""),
                );
            },
        );
        modified_col.set_fixed_width(150);
        modified_col.set_sorter(Some(&sorter(|a, b| a.modified_ts.cmp(&b.modified_ts))));

        for c in [&name_col, &size_col, &modified_col] {
            column_view.append_column(c);
        }
        column_view.sort_by_column(Some(&name_col), gtk::SortType::Ascending);

        let scrolled = gtk::ScrolledWindow::builder()
            .child(&column_view)
            .vexpand(true)
            .build();

        let empty_page = adw::StatusPage::builder()
            .icon_name("folder-symbolic")
            .build();
        let retry_button = gtk::Button::builder()
            .label("Try Again")
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        let error_page = adw::StatusPage::builder()
            .icon_name("dialog-error-symbolic")
            .title("Couldn't Load This Folder")
            .child(&retry_button)
            .build();
        let loading = gtk::Spinner::builder()
            .spinning(true)
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&scrolled, Some("list"));
        stack.add_named(&empty_page, Some("empty"));
        stack.add_named(&error_page, Some("error"));
        stack.add_named(&loading, Some("loading"));

        let drop_label = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let hint_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Fill)
            .halign(gtk::Align::Fill)
            .css_classes(["drop-hint"])
            .build();
        let inner = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Center)
            .vexpand(true)
            .build();
        inner.append(
            &gtk::Image::builder()
                .icon_name("folder-download-symbolic")
                .pixel_size(64)
                .css_classes(["accent"])
                .build(),
        );
        inner.append(&drop_label);
        hint_box.append(&inner);
        let drop_hint = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .child(&hint_box)
            .can_target(false)
            .build();

        let root = gtk::Overlay::builder().child(&stack).build();
        root.add_overlay(&drop_hint);

        let drop_target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        drop_target.connect_enter({
            let hint = drop_hint.clone();
            move |_, _, _| {
                hint.set_reveal_child(true);
                gdk::DragAction::COPY
            }
        });
        drop_target.connect_leave({
            let hint = drop_hint.clone();
            move |_| hint.set_reveal_child(false)
        });
        root.add_controller(drop_target.clone());

        Self {
            root,
            store,
            filtered,
            filter,
            filter_text,
            selection,
            column_view,
            stack,
            empty_page,
            error_page,
            retry_button,
            drop_target,
            drop_hint,
            drop_label,
        }
    }

    /// Calls `f` with the local paths of files dropped onto the list.
    pub fn connect_drop(&self, f: impl Fn(Vec<PathBuf>) + 'static) {
        let hint = self.drop_hint.clone();
        self.drop_target.connect_drop(move |_, value, _, _| {
            hint.set_reveal_child(false);
            let Ok(files) = value.get::<gdk::FileList>() else {
                return false;
            };
            let paths: Vec<PathBuf> = files.files().iter().filter_map(|f| f.path()).collect();
            if paths.is_empty() {
                return false;
            }
            f(paths);
            true
        });
    }

    /// Enables dropping, describing where dropped files will go; `None` disables it.
    pub fn set_drop_destination(&self, destination: Option<&str>) {
        match destination {
            Some(d) => {
                self.drop_target.set_actions(gdk::DragAction::COPY);
                self.drop_label.set_label(&format!("Drop to upload to {d}"));
            }
            None => self.drop_target.set_actions(gdk::DragAction::empty()),
        }
    }

    pub fn show_loading(&self) {
        self.store.remove_all();
        self.stack.set_visible_child_name("loading");
    }

    pub fn show_error(&self, message: &str) {
        self.store.remove_all();
        self.error_page.set_description(Some(message));
        self.stack.set_visible_child_name("error");
    }

    pub fn show_placeholder(&self, icon: &str, title: &str, description: &str) {
        self.store.remove_all();
        self.empty_page.set_icon_name(Some(icon));
        self.empty_page.set_title(title);
        self.empty_page.set_description(Some(description));
        self.stack.set_visible_child_name("empty");
    }

    pub fn set_entries(&self, entries: Vec<Entry>) {
        let objects: Vec<glib::BoxedAnyObject> =
            entries.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &objects);
        self.selection.unselect_all();
        self.update_placeholder();
        if self.selection.n_items() > 0 {
            self.column_view
                .scroll_to(0, None, gtk::ListScrollFlags::NONE, None);
        }
    }

    pub fn set_filter(&self, text: &str) {
        *self.filter_text.borrow_mut() = text.to_lowercase();
        self.filter.changed(gtk::FilterChange::Different);
        self.update_placeholder();
    }

    fn update_placeholder(&self) {
        if self.stack.visible_child_name().as_deref() == Some("error") && self.store.n_items() == 0
        {
            return;
        }
        if self.store.n_items() == 0 {
            self.empty_page.set_icon_name(Some("folder-symbolic"));
            self.empty_page.set_title("Empty Folder");
            self.empty_page
                .set_description(Some("Drop files here or use Upload to add some"));
            self.stack.set_visible_child_name("empty");
        } else if self.filtered.n_items() == 0 {
            self.empty_page
                .set_icon_name(Some("system-search-symbolic"));
            self.empty_page.set_title("No Results Found");
            self.empty_page
                .set_description(Some("Nothing in this folder matches the search"));
            self.stack.set_visible_child_name("empty");
        } else {
            self.stack.set_visible_child_name("list");
        }
    }

    pub fn entries(&self) -> Vec<Entry> {
        (0..self.store.n_items())
            .filter_map(|i| self.store.item(i))
            .map(|o| entry_of(&o).clone())
            .collect()
    }

    pub fn selected(&self) -> Vec<Entry> {
        (0..self.selection.n_items())
            .filter(|&i| self.selection.is_selected(i))
            .filter_map(|i| self.selection.item(i))
            .map(|o| entry_of(&o).clone())
            .collect()
    }

    pub fn entry_at(&self, position: u32) -> Option<Entry> {
        self.selection.item(position).map(|o| entry_of(&o).clone())
    }
}

/// What every cell needs to open the context menu on right-click.
#[derive(Clone)]
struct CellContext {
    selection: gtk::MultiSelection,
    menu: gtk::PopoverMenu,
    column_view: gtk::ColumnView,
}

fn column(
    ctx: &CellContext,
    title: &str,
    make: impl Fn() -> gtk::Widget + 'static,
    bind: impl Fn(&gtk::Widget, &Entry) + 'static,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let ctx = ctx.clone();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let widget = make();
        attach_context_menu(&widget, item, &ctx);
        item.set_child(Some(&widget));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        if let (Some(widget), Some(obj)) = (item.child(), item.item()) {
            bind(&widget, &entry_of(&obj));
        }
    });
    gtk::ColumnViewColumn::new(Some(title), Some(factory))
}

/// Right-click selects the row under the pointer (unless it's already part of the selection)
/// and opens the context menu there.
fn attach_context_menu(widget: &gtk::Widget, item: &gtk::ListItem, ctx: &CellContext) {
    let gesture = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();
    let item = item.downgrade();
    let ctx = ctx.clone();
    gesture.connect_pressed(move |gesture, _, x, y| {
        let Some(item) = item.upgrade() else { return };
        let pos = item.position();
        if pos == gtk::INVALID_LIST_POSITION {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if !ctx.selection.is_selected(pos) {
            ctx.selection.select_item(pos, true);
        }
        let Some(widget) = gesture.widget() else {
            return;
        };
        let Some(point) =
            widget.compute_point(&ctx.column_view, &graphene::Point::new(x as f32, y as f32))
        else {
            return;
        };
        ctx.menu.set_pointing_to(Some(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        ctx.menu.popup();
    });
    widget.add_controller(gesture);
}

fn label_cell(xalign: f32, classes: &[&str]) -> gtk::Widget {
    gtk::Label::builder()
        .xalign(xalign)
        .hexpand(true)
        .css_classes(classes.to_vec())
        .build()
        .upcast()
}

fn set_label(widget: &gtk::Widget, text: &str) {
    widget
        .downcast_ref::<gtk::Label>()
        .expect("label cell")
        .set_label(text);
}
