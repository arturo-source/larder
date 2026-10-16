//! Bottom bar listing running uploads with progress, speed, ETA and a cancel button.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use crate::s3::{Progress, human_size};

struct Row {
    progress: Arc<Progress>,
    root: gtk::Box,
    title: gtk::Label,
    file: gtk::Label,
    bar: gtk::ProgressBar,
    stats: gtk::Label,
    cancel: gtk::Button,
}

pub struct TransfersBar {
    pub root: gtk::Revealer,
    list: gtk::Box,
    rows: RefCell<Vec<Row>>,
    ticking: Cell<bool>,
}

impl TransfersBar {
    pub fn new() -> Rc<Self> {
        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["toolbar"])
            .build();
        let root = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .child(&list)
            .build();
        Rc::new(Self {
            root,
            list,
            rows: RefCell::default(),
            ticking: Cell::new(false),
        })
    }

    pub fn add(self: &Rc<Self>, progress: Arc<Progress>) {
        let title = gtk::Label::builder()
            .label(&progress.label)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let file = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["dim-label", "caption"])
            .build();
        let stats = gtk::Label::builder()
            .xalign(1.0)
            .css_classes(["dim-label", "caption", "numeric"])
            .build();
        let bar = gtk::ProgressBar::new();
        let cancel = gtk::Button::builder()
            .icon_name("process-stop-symbolic")
            .tooltip_text("Cancel Upload")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        cancel.connect_clicked({
            let p = progress.clone();
            move |b| {
                p.cancel();
                b.set_sensitive(false);
            }
        });

        let labels = gtk::Box::builder().spacing(12).build();
        labels.append(&title);
        labels.append(&file);
        labels.append(&stats);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .hexpand(true)
            .build();
        text.append(&labels);
        text.append(&bar);
        let root = gtk::Box::builder()
            .spacing(12)
            .css_classes(["transfer-row"])
            .visible(false)
            .build();
        root.append(&gtk::Image::from_icon_name("document-send-symbolic"));
        root.append(&text);
        root.append(&cancel);
        self.list.append(&root);

        self.rows.borrow_mut().push(Row {
            progress,
            root,
            title,
            file,
            bar,
            stats,
            cancel,
        });
        if !self.ticking.replace(true) {
            let this = self.clone();
            glib::timeout_add_local(Duration::from_millis(150), move || this.tick());
        }
    }

    fn tick(&self) -> glib::ControlFlow {
        let mut rows = self.rows.borrow_mut();
        rows.retain(|row| {
            let finished = row.progress.finished.load(Ordering::Relaxed);
            if finished {
                self.list.remove(&row.root);
            }
            !finished
        });
        for row in rows.iter() {
            update(row);
        }
        self.root
            .set_reveal_child(rows.iter().any(|r| r.root.is_visible()));
        if rows.is_empty() {
            self.ticking.set(false);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    }
}

fn update(row: &Row) {
    let p = &row.progress;
    let total_files = p.total_files.load(Ordering::Relaxed);
    // Nothing to show until the file dialog closed and folders were scanned.
    row.root.set_visible(total_files > 0);
    if total_files == 0 {
        return;
    }
    let total = p.total_bytes.load(Ordering::Relaxed);
    let done = p.done_bytes.load(Ordering::Relaxed);
    let done_files = p.done_files.load(Ordering::Relaxed);
    let speed = done as f64 / p.started.elapsed().as_secs_f64().max(0.001);
    let fraction = if total == 0 {
        done_files as f64 / total_files as f64
    } else {
        done as f64 / total as f64
    };
    row.bar.set_fraction(fraction);
    if let Ok(current) = p.current.lock() {
        row.file.set_label(&current);
    }
    let eta = if speed > 0.0 && total > done {
        format!(" · {} left", fmt_eta((total - done) as f64 / speed))
    } else {
        String::new()
    };
    row.stats.set_label(&format!(
        "{} of {} · {done_files}/{total_files} files · {}/s{eta}",
        human_size(done as i64),
        human_size(total as i64),
        human_size(speed as i64)
    ));
    if p.is_cancelled() {
        row.title.set_label("Cancelling…");
        row.cancel.set_sensitive(false);
    }
}

fn fmt_eta(secs: f64) -> String {
    let s = secs.ceil() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}
