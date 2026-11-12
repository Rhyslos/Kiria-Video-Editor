//! The Library page: every file of the project, by type and in the
//! person's own lists, filtered by labels and smart filters, with a detail
//! panel to look at one, mark the part wanted and add it to the timeline.
//!
//! Layout (the same top bar and tabs as every page): the sidebar on the
//! left (`sidebar`: Import, labels with counts, smart filters, Prune), the
//! tab bar and filter row on top of the middle (`top_rows`), the files
//! under them (`cards`: a thumbnail grid or rows for pictures; `sound_rows`
//! for the Audio tab), and the detail panel on the right, as wide as the
//! Edit page's inspector (`detail`).
//!
//! Finding and ordering is the library index's (`library::Live`, asked
//! under `ASKER` on its own thread, never waited for): the words, the open
//! tab's type or list, the label chips, "Not in a timeline" and the order.
//! The smart filters the index can't know (proxies, missing files) and the
//! grouping are worked out here from its answer (`arrange`), again only
//! when something changed, and only the lines in view are drawn, so
//! thousands of files scroll smoothly.

mod arrange;
mod cards;
mod detail;
pub mod loudness;
pub mod placing;
pub mod player;
mod sidebar;
mod sound_rows;
mod top_rows;

use crate::organise::{Everywhere, Organised, Organiser};
use crate::proxies::Status;
use arrange::{Arranged, Facts, Sizes};
use library::{Query, Sort, Type};
use project::{Command, GlobalLabels, MediaId, MediaKind, MediaLabelId, MediaListId, MediaRef, Project};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui_kit::scrub::Marks;
use ui_kit::timeline::LoadState;
use ui_kit::{Action, PreviewQuality, ShortcutTable, Theme, layout};

pub use placing::{AddAt, Adding};

/// The name the page asks the library index under.
pub const ASKER: library::Asker = "library page";

/// Room between the sidebar, the middle and the detail panel.
const GAP: f32 = 12.0;

/// Counts and groups are worked out again at least this often, so proxy
/// states that change on their own show.
const FRESH_FOR: Duration = Duration::from_secs(1);

/// A tab of the tab bar: a type, or one of the person's lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Type(Type),
    List(MediaListId),
}

impl Default for Tab {
    fn default() -> Tab {
        Tab::Type(Type::Video)
    }
}

/// Filters the sidebar offers besides labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Smart {
    /// On no timeline of the project.
    NotUsed,
    /// Jumps in it are slow and it has no proxy yet.
    NeedsProxy,
    /// A recording whose end was missing, imported as far as it goes.
    EndMissing,
    /// Not where the project says it is.
    Missing,
    WithNote,
}

impl Smart {
    pub const ALL: [Smart; 5] = [
        Smart::NotUsed,
        Smart::NeedsProxy,
        Smart::EndMissing,
        Smart::Missing,
        Smart::WithNote,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Smart::NotUsed => "Not in a timeline",
            Smart::NeedsProxy => "Needs proxy",
            Smart::EndMissing => "End missing",
            Smart::Missing => "Can't be found",
            Smart::WithNote => "With a note",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Smart::NotUsed => "Files no timeline of the project uses",
            Smart::NeedsProxy => "Video files that are slow to jump around in and have no proxy yet",
            Smart::EndMissing => "Recordings whose end was missing, imported as far as they go",
            Smart::Missing => {
                "Files that aren't where the project says (moved, renamed or on a drive that isn't connected)"
            }
            Smart::WithNote => "Files you wrote a note for",
        }
    }
}

/// How the files are grouped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GroupBy {
    /// Under their first label; files without one last.
    #[default]
    Label,
    /// By the folder they are in.
    Folder,
    Nothing,
}

impl GroupBy {
    pub const ALL: [GroupBy; 3] = [GroupBy::Label, GroupBy::Folder, GroupBy::Nothing];

    pub fn name(self) -> &'static str {
        match self {
            GroupBy::Label => "Label",
            GroupBy::Folder => "Folder",
            GroupBy::Nothing => "Nothing",
        }
    }
}

/// Pictures as a grid of thumbnails, or as rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum View {
    #[default]
    Grid,
    Rows,
}

/// What was asked for on the page, for the window to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Picked {
    /// A change to the project's labels, lists or notes: one undo step.
    Edit(Box<Command>),
    /// A change to the labels offered in every project.
    Everywhere(Everywhere),
    /// Add a file, or part of it, to the open timeline.
    Add(Adding),
    BuildProxy(MediaId),
    DeleteProxy(MediaId),
    /// Open the import dialog.
    Import,
}

impl From<Organised> for Picked {
    fn from(organised: Organised) -> Picked {
        match organised {
            Organised::Edit(command) => Picked::Edit(command),
            Organised::Everywhere(change) => Picked::Everywhere(change),
        }
    }
}

/// What the page is told besides its own state.
pub struct About<'a> {
    pub project: &'a Project,
    /// The library index's newest answer to the page (`None` until it has one).
    pub answer: Option<Arc<library::Answer>>,
    /// Files the project has that aren't on disk.
    pub missing: &'a [MediaId],
    pub proxy: &'a dyn Fn(MediaId) -> Status,
    pub proxy_mode: cache::proxy::ProxyMode,
    /// The labels offered in every project.
    pub global: &'a GlobalLabels,
    /// The loudness aimed for, LUFS (none when nothing is aimed for).
    pub target: Option<f32>,
    pub shortcuts: &'a ShortcutTable,
    pub quality: PreviewQuality,
    pub jobs: &'a jobs::JobSystem,
    /// Files being checked for import now.
    pub importing: usize,
    /// The window's status line.
    pub status: &'a str,
}

/// What reading a file's pictures gives: them, once read, and how far the reading is.
pub type PictureLook = (Option<ui_kit::ClipPictures>, Option<LoadState>);

/// Reading what files hold, for their thumbnails and waveforms: started
/// the first time a file is in view, kept for the session.
pub struct Readers<'a> {
    pub pictures: &'a mut dyn FnMut(&MediaRef, u32) -> PictureLook,
    pub waveform: &'a mut dyn FnMut(&MediaRef, u32) -> crate::waveforms::Look,
}

/// The page's state, kept by the window.
pub struct LibraryPage {
    pub tab: Tab,
    /// Words to find, in names, labels and notes.
    pub text: String,
    /// Only files with every one of these labels (the chips).
    pub labels: Vec<MediaLabelId>,
    pub smart: Option<Smart>,
    pub group: GroupBy,
    pub sort: Sort,
    /// How wide a thumbnail is, in points (also how tall sound rows are).
    pub size: f32,
    pub view: View,
    /// The file in the detail panel.
    pub selected: Option<MediaId>,
    /// The part of each file marked to add, for the session.
    marks: HashMap<MediaId, Marks>,
    /// A level chosen in the detail panel to add a file at (thousandths of a dB).
    levels: HashMap<MediaId, i32>,
    pub player: player::Player,
    loudness: loudness::Measurements,
    organiser: Organiser,
    sidebar: sidebar::Drafts,
    top: top_rows::Drafts,
    /// The note being written in the detail panel.
    note: Option<(MediaId, String)>,
    facts: Option<(Kept, Facts)>,
    arranged: Option<(Kept, Arc<Arranged>)>,
    /// Files in view still waiting for their picture or waveform, and
    /// whether the detail panel waits for anything (for screenshots).
    waiting: usize,
    /// `--library-select`: select the file with this name once it is in.
    select_named: Option<String>,
}

impl Default for LibraryPage {
    fn default() -> LibraryPage {
        LibraryPage {
            tab: Tab::default(),
            text: String::new(),
            labels: Vec::new(),
            smart: None,
            group: GroupBy::default(),
            sort: Sort::default(),
            size: cards::USUAL_SIZE,
            view: View::default(),
            selected: None,
            marks: HashMap::new(),
            levels: HashMap::new(),
            player: player::Player::default(),
            loudness: loudness::Measurements::default(),
            organiser: Organiser::default(),
            sidebar: sidebar::Drafts::default(),
            top: top_rows::Drafts::default(),
            note: None,
            facts: None,
            arranged: None,
            waiting: 0,
            select_named: None,
        }
    }
}

/// What counts and groups were worked out from, to tell when to work them
/// out again (the project's parts are compared by pointer: free).
#[derive(Clone)]
struct Kept {
    answer: usize,
    tab: Tab,
    smart: Option<Smart>,
    group: GroupBy,
    sizes: Option<Sizes>,
    project: Project,
    missing: usize,
    made: Instant,
}

impl Kept {
    fn same(&self, other: &Kept) -> bool {
        let (a, b) = (&self.project, &other.project);
        self.answer == other.answer
            && self.tab == other.tab
            && self.smart == other.smart
            && self.group == other.group
            && self.sizes == other.sizes
            && self.missing == other.missing
            && a.media.ptr_eq(&b.media)
            && a.sequences.ptr_eq(&b.sequences)
            && a.library.labels.ptr_eq(&b.library.labels)
            && a.library.tagged.ptr_eq(&b.library.tagged)
            && a.library.notes.ptr_eq(&b.library.notes)
            && a.library.lists.ptr_eq(&b.library.lists)
            && other.made.duration_since(self.made) < FRESH_FOR
    }
}

impl LibraryPage {
    /// A page that selects the file called `name` once it is in (`--library-select`).
    pub fn selecting(name: Option<String>) -> LibraryPage {
        LibraryPage {
            select_named: name,
            ..LibraryPage::default()
        }
    }

    /// Opens a tab by its name: a type ("video", "audio", "images",
    /// "effects") or a list of the project. Returns whether one was found.
    pub fn open_tab_named(&mut self, name: &str, project: &Project) -> bool {
        let wanted = name.to_lowercase();
        if let Some(t) = Type::ALL.iter().find(|t| t.name().to_lowercase() == wanted) {
            self.tab = Tab::Type(*t);
            return true;
        }
        match project
            .library
            .lists
            .iter()
            .find(|l| l.name.to_lowercase() == wanted)
        {
            Some(list) => {
                self.tab = Tab::List(list.id);
                true
            }
            None => false,
        }
    }

    /// What the page asks the library index for now. A label or list that
    /// no longer exists (undone, removed) asks for nothing of it.
    pub fn query(&self, project: &Project) -> Query {
        let (types, list) = match self.tab {
            Tab::Type(t) => (vec![t], None),
            Tab::List(l) => (Vec::new(), project.library.list(l).map(|l| l.id)),
        };
        Query {
            text: self.text.clone(),
            types,
            labels: self
                .labels
                .iter()
                .copied()
                .filter(|l| project.library.label(*l).is_some())
                .collect(),
            list,
            usage: match self.smart {
                Some(Smart::NotUsed) => library::Usage::Unused,
                _ => library::Usage::Any,
            },
            sort: if self.sort == Sort::ListOrder && list.is_none() {
                Sort::Name
            } else {
                self.sort
            },
        }
    }

    /// Keeps the page's choices about things that still exist: a list tab
    /// whose list was removed goes back to Video, gone labels leave the
    /// chips, a gone file leaves the panel.
    fn keep_existing(&mut self, project: &Project) {
        if let Tab::List(l) = self.tab
            && project.library.list(l).is_none()
        {
            self.tab = Tab::default();
        }
        self.labels.retain(|l| project.library.label(*l).is_some());
        if self.selected.is_some_and(|m| !project.media.contains_key(&m)) {
            self.selected = None;
        }
    }

    /// The marked part of `media`.
    pub fn marks(&self, media: MediaId) -> Marks {
        self.marks.get(&media).copied().unwrap_or_default()
    }

    /// What "Add to timeline" adds for `media`: its marked part, at the level chosen for it.
    pub fn adding(&self, media: &MediaRef, at: AddAt) -> Adding {
        let marks = self.marks(media.id);
        Adding {
            media: media.id,
            part: marks.is_set().then(|| marks.part(media.duration.as_secs_f64())),
            gain_mdb: self.levels.get(&media.id).copied().unwrap_or(0),
            at,
        }
    }

    /// A command from the keys while the page is open: the player's
    /// (play, stop, frames, start and end) and marking in and out. Returns
    /// whether the page took it.
    pub fn command(&mut self, action: Action) -> bool {
        let Some(media) = self.player.media() else {
            return matches!(
                action,
                Action::PlayPause
                    | Action::Stop
                    | Action::PreviousFrame
                    | Action::NextFrame
                    | Action::GoToStart
                    | Action::GoToEnd
                    | Action::MarkIn
                    | Action::MarkOut
            );
        };
        let marks = self.marks(media);
        let length = self.player.length();
        match action {
            Action::PlayPause => self.player.play_pause(marks),
            Action::Stop => self.player.stop_here(),
            Action::PreviousFrame => self.player.step(-1, marks),
            Action::NextFrame => self.player.step(1, marks),
            Action::GoToStart => self.player.go_to(0.0, marks),
            Action::GoToEnd => self.player.go_to(length, marks),
            Action::MarkIn => self.marks.entry(media).or_default().set_in(self.player.at),
            Action::MarkOut => self.marks.entry(media).or_default().set_out(self.player.at),
            _ => return false,
        }
        true
    }

    /// Whether everything in view has what it shows (for screenshots).
    pub fn settled(&self) -> bool {
        self.waiting == 0 && self.player.settled()
    }

    /// Every frame, on any page: the player follows its clock. Returns an
    /// error to show.
    pub fn poll(&mut self, ctx: &egui::Context, quality: PreviewQuality) -> Option<diag::AppError> {
        self.player.poll(ctx, quality)
    }

    /// The page was left: the player stops.
    pub fn leave(&mut self) {
        self.player.stop_here();
    }

    /// The app is closing.
    pub fn close(&mut self) {
        self.player.close();
        self.loudness.stop();
    }

    /// A file was found somewhere else: measure it again.
    pub fn forget(&mut self, media: MediaId) {
        self.loudness.forget(media);
    }

    /// Draws the page. Returns what was asked for.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        about: &About<'_>,
        readers: &mut Readers<'_>,
    ) -> Vec<Picked> {
        let project = about.project;
        self.keep_existing(project);
        if let Some(name) = self.select_named.clone()
            && let Some(media) = project
                .media
                .values()
                .find(|m| library::file_name(&m.path) == name)
        {
            self.selected = Some(media.id);
            self.select_named = None;
        }
        let selected = self.selected.and_then(|id| project.media.get(&id));
        self.player.show(
            selected,
            selected.map_or_else(Marks::default, |m| self.marks(m.id)),
        );
        self.waiting = 0;
        let page = ui.available_rect_before_wrap();
        let left = egui::Rect::from_min_size(page.min, egui::vec2(layout::LEFT_PANEL_WIDTH, page.height()));
        let right = egui::Rect::from_min_max(
            egui::pos2(page.right() - layout::RIGHT_PANEL_WIDTH, page.top()),
            page.max,
        );
        let middle = egui::Rect::from_min_max(
            egui::pos2(left.right() + GAP, page.top()),
            egui::pos2(right.left() - GAP, page.bottom()),
        );
        let facts = self.facts(about);
        let mut picked = Vec::new();
        panel(ui, theme, left, |ui| {
            picked.extend(sidebar::ui(self, ui, theme, about, &facts));
        });
        let mut column = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(middle)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        picked.extend(top_rows::ui(self, &mut column, theme, about, &facts));
        column.add_space(8.0);
        picked.extend(self.files(&mut column, theme, about, &facts, readers));
        panel(ui, theme, right, |ui| {
            picked.extend(detail::ui(self, ui, theme, about, &facts, readers));
        });
        ui.allocate_rect(page, egui::Sense::hover());
        picked
    }

    /// The counts of the sidebar and the tabs, worked out again when
    /// something they count changed.
    fn facts(&mut self, about: &About<'_>) -> Facts {
        let kept = self.kept(about, None);
        if let Some((was, facts)) = &self.facts
            && was.same(&kept)
        {
            return facts.clone();
        }
        let missing: HashSet<MediaId> = about.missing.iter().copied().collect();
        let needs = |m: MediaId| (about.proxy)(m) == Status::Needed;
        let facts = Facts::of(about.project, self.tab, &needs, &missing);
        self.facts = Some((kept, facts.clone()));
        facts
    }

    fn kept(&self, about: &About<'_>, sizes: Option<Sizes>) -> Kept {
        Kept {
            answer: about.answer.as_ref().map_or(0, |a| Arc::as_ptr(a) as usize),
            tab: self.tab,
            smart: self.smart,
            group: self.group,
            sizes,
            project: about.project.clone(),
            missing: about.missing.len(),
            made: Instant::now(),
        }
    }

    /// The files found, filtered by the smart filter and grouped, in lines
    /// of `sizes`; worked out again only when something changed.
    fn arranged(&mut self, about: &About<'_>, facts: &Facts, sizes: Sizes) -> Arc<Arranged> {
        let kept = self.kept(about, Some(sizes));
        if let Some((was, arranged)) = &self.arranged
            && was.same(&kept)
        {
            return Arc::clone(arranged);
        }
        let project = about.project;
        let missing: HashSet<MediaId> = about.missing.iter().copied().collect();
        let needs = |m: MediaId| (about.proxy)(m) == Status::Needed;
        // An answer for another tab (the index is still on its way to this
        // one's) isn't shown; until it comes, the tab's files by name.
        let asked = self.query(project);
        let answer = about
            .answer
            .as_ref()
            .filter(|a| a.query.types == asked.types && a.query.list == asked.list);
        let found: Vec<MediaId> = match answer {
            Some(answer) => answer.media.clone(),
            None => {
                let mut files: Vec<&MediaRef> = project
                    .media
                    .values()
                    .filter(|m| self.tab.holds(project, m))
                    .collect();
                files.sort_by_key(|m| library::file_name(&m.path).to_lowercase());
                files.iter().map(|m| m.id).collect()
            }
        };
        let kept_files: Vec<MediaId> = found
            .into_iter()
            .filter(|id| match (self.smart, project.media.get(id)) {
                (Some(smart), Some(media)) => smart.matches(project, media, &facts.clips, &needs, &missing),
                (None, Some(_)) => true,
                (_, None) => false,
            })
            .collect();
        let arranged = Arc::new(arrange::arrange(&kept_files, project, self.group, sizes));
        self.arranged = Some((kept, Arc::clone(&arranged)));
        arranged
    }

    /// The middle of the page under the filter row: the files of the tab.
    fn files(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        about: &About<'_>,
        facts: &Facts,
        readers: &mut Readers<'_>,
    ) -> Vec<Picked> {
        let project = about.project;
        let status_room = if about.status.is_empty() { 0.0 } else { 24.0 };
        let room = egui::vec2(
            ui.available_width(),
            (ui.available_height() - status_room).max(0.0),
        );
        let mut picked = Vec::new();
        if self.tab == Tab::Type(Type::Effect) {
            empty(ui, theme, room, "Effects arrive with the effects library.", None);
            return picked;
        }
        if facts.in_tab == 0 {
            let text = match self.tab {
                Tab::List(_) => {
                    "This list is empty. Right-click a file, or use Lists in the panel on the right, to put files in it."
                }
                Tab::Type(_) if project.media.is_empty() => {
                    "Imported files show here. Import media, or drop files on the window."
                }
                Tab::Type(_) => "The project has no files of this type yet.",
            };
            empty(ui, theme, room, text, None);
            return picked;
        }
        let sound = self.tab == Tab::Type(Type::Audio);
        let sizes = if sound {
            sound_rows::sizes(self.size)
        } else {
            cards::sizes(self.view, self.size, room.x)
        };
        let arranged = self.arranged(about, facts, sizes);
        if arranged.items.is_empty() {
            let cleared = empty(
                ui,
                theme,
                room,
                "No file here matches these filters.",
                Some("Clear filters"),
            );
            if cleared {
                self.text.clear();
                self.labels.clear();
                self.smart = None;
            }
            return picked;
        }
        let scroll_id = ("library-files", self.tab);
        egui::ScrollArea::vertical()
            .id_salt(scroll_id)
            .auto_shrink([false, false])
            .max_height(room.y)
            .show_viewport(ui, |ui, viewport| {
                ui.set_height(arranged.height);
                let origin = ui.max_rect().min;
                let width = ui.available_width();
                for i in arranged.in_view(viewport.min.y, viewport.max.y, sizes) {
                    let top = origin.y + arranged.tops[i];
                    match &arranged.lines[i] {
                        arrange::Line::Heading { title, colour, count } => {
                            let rect = egui::Rect::from_min_size(
                                egui::pos2(origin.x, top),
                                egui::vec2(width, sizes.heading),
                            );
                            heading(ui, theme, rect, title, *colour, *count);
                        }
                        arrange::Line::Files { from, to } => {
                            let rect = egui::Rect::from_min_size(
                                egui::pos2(origin.x, top),
                                egui::vec2(width, sizes.row),
                            );
                            let files = &arranged.items[*from..*to];
                            let line = if sound {
                                sound_rows::row(self, ui, theme, about, facts, readers, rect, files[0])
                            } else {
                                cards::line(self, ui, theme, about, facts, readers, rect, files, sizes)
                            };
                            picked.extend(line);
                        }
                    }
                }
            });
        if !about.status.is_empty() {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(about.status)
                    .size(12.0)
                    .color(theme.text_secondary),
            );
        }
        picked
    }

    /// A click on a file: it shows in the detail panel. A double-click adds
    /// it (its marked part) at the end of the timeline.
    fn clicked(&mut self, media: &MediaRef, response: &egui::Response) -> Option<Picked> {
        if response.clicked() || response.secondary_clicked() {
            self.selected = Some(media.id);
        }
        response
            .double_clicked()
            .then(|| Picked::Add(self.adding(media, AddAt::End)))
    }

    /// A file's right-click menu: add it, its proxy, its labels, lists and note.
    fn file_menu(
        &mut self,
        response: &egui::Response,
        about: &About<'_>,
        media: &MediaRef,
    ) -> Option<Picked> {
        let mut picked = None;
        response.context_menu(|ui| {
            let marked = self.marks(media.id).is_set();
            let (end, insert) = if marked {
                (
                    "Add the marked part at the end",
                    "Insert the marked part at the edit point",
                )
            } else {
                ("Add at the end of the timeline", "Insert at the edit point")
            };
            if ui.button(end).clicked() {
                picked = Some(Picked::Add(self.adding(media, AddAt::End)));
                ui.close();
            }
            if ui.button(insert).clicked() {
                picked = Some(Picked::Add(self.adding(media, AddAt::EditPoint)));
                ui.close();
            }
            let missing = about.missing.contains(&media.id);
            if let Some(asked) =
                crate::media_drawer::proxy_menu(ui, media.id, &(about.proxy)(media.id), missing)
            {
                picked = Some(match asked {
                    crate::media_drawer::Picked::DeleteProxy(m) => Picked::DeleteProxy(m),
                    _ => Picked::BuildProxy(media.id),
                });
                ui.close();
            }
            ui.separator();
            if let Some(asked) = self.organiser.menus(ui, about.project, about.global, media.id) {
                picked = Some(asked.into());
                ui.close();
            }
        });
        picked
    }
}

/// A panel of the page (the sidebar, the detail panel) in `rect`, framed as
/// the Edit page's inspector is.
fn panel(ui: &mut egui::Ui, theme: &Theme, rect: egui::Rect, add: impl FnOnce(&mut egui::Ui)) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    egui::Frame::new()
        .fill(theme.panel)
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(&mut child, |ui| {
            ui.set_min_size(rect.size() - egui::vec2(24.0, 20.0));
            ui.set_max_size(rect.size() - egui::vec2(24.0, 20.0));
            add(ui);
        });
}

/// A group's heading: its colour, name and how many files are in it.
fn heading(
    ui: &egui::Ui,
    theme: &Theme,
    rect: egui::Rect,
    title: &str,
    colour: Option<project::LabelColor>,
    count: usize,
) {
    let painter = ui.painter_at(rect);
    let mut x = rect.left() + 2.0;
    let y = rect.center().y + 2.0;
    if let Some(colour) = colour {
        painter.circle_filled(
            egui::pos2(x + 4.0, y),
            4.5,
            crate::timeline_panel::label_colour(colour),
        );
        x += 14.0;
    }
    let name = painter.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::new(13.0, ui_kit::fonts::semibold()),
        theme.text,
    );
    painter.text(
        egui::pos2(name.right() + 8.0, y),
        egui::Align2::LEFT_CENTER,
        count.to_string(),
        egui::FontId::proportional(12.0),
        theme.muted,
    );
    painter.line_segment(
        [
            egui::pos2(name.right() + 30.0, y),
            egui::pos2(rect.right() - 4.0, y),
        ],
        egui::Stroke::new(1.0, theme.line),
    );
}

/// A quiet line where files would be, with a button when `button` names
/// one. Returns whether it was clicked.
fn empty(ui: &mut egui::Ui, theme: &Theme, room: egui::Vec2, text: &str, button: Option<&str>) -> bool {
    let mut clicked = false;
    ui.allocate_ui(room, |ui| {
        ui.set_min_size(room);
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(text).color(theme.muted));
            if let Some(button) = button {
                ui.add_space(6.0);
                clicked = ui.button(button).clicked();
            }
        });
    });
    clicked
}

/// The picture stream a file's thumbnail is read from: video files and pictures.
fn picture_stream(media: &MediaRef) -> Option<u32> {
    (media.kind != MediaKind::Audio)
        .then_some(media.info.video.as_ref())
        .flatten()
        .map(|v| v.index)
}

/// A file's length and what it holds, in a few words: "0:42 · 1080p 60 h264".
fn details(media: &MediaRef) -> String {
    let summary = crate::import::summary(media);
    let rest = summary.split_once(" · ").map_or("", |(_, rest)| rest);
    if media.kind == MediaKind::Image {
        return rest.to_owned();
    }
    let length = ui_kit::time_text::length(media.duration.as_secs_f64());
    if rest.is_empty() {
        length
    } else {
        format!("{length} · {rest}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{LabelColor, MediaLabel};

    #[test]
    fn the_query_follows_the_tab_chips_and_filters() {
        let p = Project::new_automatic("T");
        let p = Command::AddMediaLabel {
            label: MediaLabel {
                id: MediaLabelId(0),
                name: "Drone".into(),
                color: LabelColor::Green,
            },
            media: vec![],
        }
        .apply(&p)
        .unwrap();
        let p = Command::AddMediaList {
            name: "Opening".into(),
            media: vec![],
        }
        .apply(&p)
        .unwrap();
        let drone = p.library.labels[0].id;
        let opening = p.library.lists[0].id;
        let mut page = LibraryPage::default();
        assert_eq!(page.query(&p).types, [Type::Video]);
        page.labels = vec![drone, MediaLabelId(999)];
        page.smart = Some(Smart::NotUsed);
        page.sort = Sort::ListOrder;
        let q = page.query(&p);
        assert_eq!(q.labels, [drone], "a gone label asks for nothing");
        assert_eq!(q.usage, library::Usage::Unused);
        assert_eq!(q.sort, Sort::Name, "list order only in a list");
        assert!(page.open_tab_named("opening", &p));
        let q = page.query(&p);
        assert_eq!(
            (q.types.len(), q.list),
            (0, Some(opening)),
            "a list holds every type"
        );
        assert_eq!(q.sort, Sort::ListOrder);
        assert!(page.open_tab_named("Audio", &p));
        assert_eq!(page.tab, Tab::Type(Type::Audio));
        assert!(!page.open_tab_named("nothing like it", &p));
        // The list goes: back to Video.
        page.tab = Tab::List(opening);
        let p = Command::RemoveMediaList { list: opening }.apply(&p).unwrap();
        page.keep_existing(&p);
        assert_eq!(page.tab, Tab::default());
    }

    #[test]
    fn keys_mark_the_part_of_the_file_in_the_player() {
        let mut page = LibraryPage::default();
        assert!(
            page.command(Action::MarkIn),
            "taken even with no file, so the timeline isn't touched"
        );
        assert!(!page.command(Action::Split));
        let media = MediaRef {
            id: MediaId(3),
            path: "/r/a.wav".into(),
            fingerprint: project::Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: MediaKind::Audio,
            duration: project::Time::seconds(10),
            info: project::MediaInfo::default(),
        };
        page.player.show(Some(&media), Marks::default());
        page.player.at = 2.0;
        page.command(Action::MarkIn);
        page.player.at = 6.0;
        page.command(Action::MarkOut);
        assert_eq!(page.marks(media.id).part(10.0), (2.0, 6.0));
        let adding = page.adding(&media, AddAt::End);
        assert_eq!(adding.part, Some((2.0, 6.0)));
        assert_eq!(adding.gain_mdb, 0);
        page.levels.insert(media.id, -1_500);
        assert_eq!(page.adding(&media, AddAt::End).gain_mdb, -1_500);
    }

    #[test]
    fn details_say_length_and_contents() {
        let media = MediaRef {
            id: MediaId(3),
            path: "/r/a.wav".into(),
            fingerprint: project::Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: MediaKind::Audio,
            duration: project::Time::seconds(42),
            info: project::MediaInfo::default(),
        };
        assert_eq!(details(&media), "42.0 s · no sound");
    }
}
