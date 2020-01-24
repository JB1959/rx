use crate::resources::SnapshotId;
use crate::session::{Session, SessionCoords};
use crate::util;

use rgx::kit::Animation;
use rgx::kit::Rgba8;
use rgx::math::*;
use rgx::rect::Rect;

use nonempty::NonEmpty;

use std::collections::btree_map;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time;

/// View identifier.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Copy, Clone, Debug)]
pub struct ViewId(u16);

impl Default for ViewId {
    fn default() -> Self {
        ViewId(0)
    }
}

impl fmt::Display for ViewId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// View coordinates.
///
/// These coordinates are relative to the bottom left corner of the view.
#[derive(Copy, Clone, PartialEq)]
pub struct ViewCoords<T>(Point2<T>);

impl<T> ViewCoords<T> {
    pub fn new(x: T, y: T) -> Self {
        Self(Point2::new(x, y))
    }
}

impl ViewCoords<i32> {
    pub fn clamp(&mut self, rect: Rect<i32>) {
        util::clamp(&mut self.0, rect);
    }
}

impl<T> Deref for ViewCoords<T> {
    type Target = Point2<T>;

    fn deref(&self) -> &Point2<T> {
        &self.0
    }
}

impl Into<ViewCoords<i32>> for ViewCoords<f32> {
    fn into(self) -> ViewCoords<i32> {
        ViewCoords::new(self.x.round() as i32, self.y.round() as i32)
    }
}

impl Into<ViewCoords<f32>> for ViewCoords<i32> {
    fn into(self) -> ViewCoords<f32> {
        ViewCoords::new(self.x as f32, self.y as f32)
    }
}

impl Into<ViewCoords<u32>> for ViewCoords<f32> {
    fn into(self) -> ViewCoords<u32> {
        ViewCoords::new(self.x.round() as u32, self.y.round() as u32)
    }
}

impl From<Point2<f32>> for ViewCoords<f32> {
    fn from(p: Point2<f32>) -> ViewCoords<f32> {
        ViewCoords::new(p.x, p.y)
    }
}

/// View extent information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewExtent {
    /// Frame width.
    pub fw: u32,
    /// Frame height.
    pub fh: u32,
    /// Number of frames.
    pub nframes: usize,
}

impl ViewExtent {
    pub fn new(fw: u32, fh: u32, nframes: usize) -> Self {
        ViewExtent { fw, fh, nframes }
    }

    /// Extent total width.
    pub fn width(&self) -> u32 {
        self.fw * self.nframes as u32
    }

    /// Extent total height.
    pub fn height(&self) -> u32 {
        self.fh
    }

    /// Rect containing the whole extent.
    pub fn rect(&self) -> Rect<u32> {
        Rect::origin(self.width(), self.height())
    }

    /// Rect containing a single frame.
    pub fn frame(&self, n: usize) -> Rect<u32> {
        let n = n as u32;
        Rect::new(self.fw * n, 0, self.fw * n + self.fw, self.fh)
    }
}

/// Current state of the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewState {
    /// The view is okay. It doesn't need to be redrawn or saved.
    Okay,
    /// The view has been touched, the changes need to be stored in a snapshot.
    /// If the parameter is `Some`, the view extents were changed.
    Dirty,
    /// The view is damaged, it needs to be redrawn from a snapshot.
    /// This happens when undo/redo is used.
    Damaged(ViewExtent),
}

/// A view operation to be carried out by the renderer.
#[derive(Debug, Clone)]
pub enum ViewOp {
    /// Copy an area of the view to another area.
    Blit(Rect<f32>, Rect<f32>),
    /// Clear to a color.
    Clear(Rgba8),
    /// Yank the given area into the paste buffer.
    Yank(Rect<i32>),
    /// Blit the paste buffer into the given area.
    Paste(Rect<i32>),
    /// Resize the view.
    Resize(u32, u32),
}

/// A view on a sprite or image.
#[derive(Debug)]
pub struct View {
    /// Frame width.
    pub fw: u32,
    /// Frame height.
    pub fh: u32,
    /// View offset relative to the session workspace.
    pub offset: Vector2<f32>,
    /// Identifier.
    pub id: ViewId,
    /// Zoom level.
    pub zoom: f32,
    /// List of operations to carry out on the view.  Cleared every frame.
    pub ops: Vec<ViewOp>,
    /// Whether the view is flipped in the X axis.
    pub flip_x: bool,
    /// Whether the view is flipped in the Y axis.
    pub flip_y: bool,
    /// Status of the file displayed by this view.
    pub file_status: FileStatus,
    /// State of the view.
    pub state: ViewState,
    /// Animation state of the sprite displayed by this view.
    pub animation: Animation<Rect<f32>>,

    /// Which view snapshot has been saved to disk, if any.
    saved_snapshot: Option<SnapshotId>,
}

impl View {
    /// Create a new view. Takes a frame width and height.
    pub fn new(id: ViewId, fs: FileStatus, fw: u32, fh: u32, nframes: usize, delay: u64) -> Self {
        let saved_snapshot = if let FileStatus::Saved(_) = &fs {
            Some(SnapshotId::default())
        } else {
            None
        };

        let origin = Rect::origin(fw as f32, fh as f32);
        let frames: Vec<_> = (0..nframes)
            .map(|i| origin + Vector2::new(i as f32 * fw as f32, 0.))
            .collect();

        Self {
            id,
            fw,
            fh,
            offset: Vector2::zero(),
            zoom: 1.,
            ops: Vec::new(),
            flip_x: false,
            flip_y: false,
            file_status: fs,
            animation: Animation::new(&frames, time::Duration::from_millis(delay)),
            state: ViewState::Okay,
            saved_snapshot,
        }
    }

    /// View width. Basically frame-width times number of frames.
    pub fn width(&self) -> u32 {
        self.fw * self.animation.len() as u32
    }

    /// View height.
    pub fn height(&self) -> u32 {
        self.fh
    }

    /// View width and height.
    pub fn size(&self) -> (u32, u32) {
        (self.width(), self.height())
    }

    /// View file name, if any.
    pub fn file_storage(&self) -> Option<&FileStorage> {
        match self.file_status {
            FileStatus::New(ref f) => Some(f),
            FileStatus::Modified(ref f) => Some(f),
            FileStatus::Saved(ref f) => Some(f),
            FileStatus::NoFile => None,
        }
    }

    /// Mark the view as saved at a specific snapshot and with
    /// the given path.
    pub fn save_as(&mut self, id: SnapshotId, storage: FileStorage) {
        match self.file_status {
            FileStatus::Modified(ref curr_fs) | FileStatus::New(ref curr_fs) => {
                if curr_fs == &storage {
                    self.saved(id, storage);
                }
            }
            FileStatus::NoFile => {
                self.saved(id, storage);
            }
            FileStatus::Saved(_) => {}
        }
    }

    /// Extend the view by one frame.
    pub fn extend(&mut self) {
        let w = self.width() as f32;
        let fw = self.fw as f32;
        let fh = self.fh as f32;

        self.animation.push_frame(Rect::new(w, 0., w + fw, fh));

        self.resized();
    }

    /// Shrink the view by one frame.
    pub fn shrink(&mut self) {
        // Don't allow the view to have zero frames.
        if self.animation.len() > 1 {
            self.animation.pop_frame();
            self.resized();
        }
    }

    /// Extend the view by one frame, by cloning an existing frame,
    /// by index.
    pub fn extend_clone(&mut self, index: i32) {
        let width = self.width() as f32;
        let (fw, fh) = (self.fw as f32, self.fh as f32);

        let index = if index == -1 {
            self.animation.len() - 1
        } else {
            index as usize
        };

        self.extend();
        self.ops.push(ViewOp::Blit(
            Rect::new(fw * index as f32, 0., fw * (index + 1) as f32, fh),
            Rect::new(width, 0., width + fw, fh),
        ));
    }

    /// Resize view frames to the given size.
    pub fn resize_frames(&mut self, fw: u32, fh: u32) {
        self.reset(ViewExtent::new(fw, fh, self.animation.len()));
        self.resized();
    }

    /// Clear the view to a color.
    pub fn clear(&mut self, color: Rgba8) {
        self.ops.push(ViewOp::Clear(color));
        self.touch();
    }

    pub fn yank(&mut self, area: Rect<i32>) {
        self.ops.push(ViewOp::Yank(area));
    }

    pub fn paste(&mut self, area: Rect<i32>) {
        self.ops.push(ViewOp::Paste(area));
        self.touch();
    }

    /// Slice the view into the given number of frames.
    pub fn slice(&mut self, nframes: usize) -> bool {
        if nframes > 0 && self.width() % nframes as u32 == 0 {
            let fw = self.width() / nframes as u32;
            self.reset(ViewExtent::new(fw, self.fh, nframes));
            // FIXME: This is very inefficient. Since the actual frame contents
            // haven't changed, we don't need to create a full snapshot. We just
            // have to record how many frames are in this snapshot.
            self.touch();

            return true;
        }
        false
    }

    /// Restore a view to a given snapshot and extent.
    pub fn restore(&mut self, sid: SnapshotId, extent: ViewExtent) {
        self.reset(extent);
        self.damaged();

        // If the snapshot was saved to disk, we mark the view as saved too.
        // Otherwise, if the view was saved before restoring the snapshot,
        // we mark it as modified.
        match self.file_status {
            FileStatus::Modified(ref f) if self.is_snapshot_saved(sid) => {
                self.file_status = FileStatus::Saved(f.clone());
            }
            FileStatus::Saved(ref f) => {
                self.file_status = FileStatus::Modified(f.clone());
            }
            _ => {
                // TODO
            }
        }
    }

    #[allow(dead_code)]
    pub fn play_animation(&mut self) {
        self.animation.play();
    }

    #[allow(dead_code)]
    pub fn pause_animation(&mut self) {
        self.animation.pause();
    }

    #[allow(dead_code)]
    pub fn stop_animation(&mut self) {
        self.animation.stop();
    }

    /// Set the delay between animation frames.
    pub fn set_animation_delay(&mut self, ms: u64) {
        self.animation.delay = time::Duration::from_millis(ms);
    }

    /// Set the view state to `Okay`.
    pub fn okay(&mut self) {
        self.state = ViewState::Okay;
    }

    /// Update the view by one "tick".
    pub fn update(&mut self, delta: time::Duration) {
        self.animation.step(delta);
    }

    /// Return the view area, including the offset.
    pub fn rect(&self) -> Rect<f32> {
        Rect::new(
            self.offset.x,
            self.offset.y,
            self.offset.x + self.width() as f32 * self.zoom,
            self.offset.y + self.height() as f32 * self.zoom,
        )
    }

    /// Check whether the session coordinates are contained within the view.
    pub fn contains(&self, p: SessionCoords) -> bool {
        self.rect().contains(*p)
    }

    /// Get the center of the view.
    pub fn center(&self) -> ViewCoords<f32> {
        ViewCoords::new(self.width() as f32 / 2., self.height() as f32 / 2.)
    }

    /// View has been modified. Called when using the brush on the view,
    /// or resizing the view.
    pub fn touch(&mut self) {
        if let FileStatus::Saved(ref f) = self.file_status {
            self.file_status = FileStatus::Modified(f.clone());
        }
        if self.state == ViewState::Okay {
            self.state = ViewState::Dirty;
        }
    }

    /// View should be considered damaged and needs to be restored from snapshot.
    /// Used when undoing or redoing changes.
    pub fn damaged(&mut self) {
        self.state = ViewState::Damaged(self.extent());
    }

    /// Check whether the view is damaged.
    pub fn is_damaged(&self) -> bool {
        if let ViewState::Damaged(_) = self.state {
            true
        } else {
            false
        }
    }

    /// Check whether the view is dirty.
    pub fn is_dirty(&self) -> bool {
        self.state == ViewState::Dirty
    }

    /// Check whether the view is okay.
    pub fn is_okay(&self) -> bool {
        self.state == ViewState::Okay
    }

    /// Return the file status as a string.
    pub fn status(&self) -> String {
        self.file_status.to_string()
    }

    /// Return the view extent.
    pub fn extent(&self) -> ViewExtent {
        ViewExtent::new(self.fw, self.fh, self.animation.len())
    }

    /// Return the view bounds, as an origin-anchored rectangle.
    pub fn bounds(&self) -> Rect<i32> {
        Rect::origin(self.width() as i32, self.height() as i32)
    }

    ////////////////////////////////////////////////////////////////////////////

    fn resized(&mut self) {
        self.touch();
        self.ops.push(ViewOp::Resize(self.width(), self.height()));
    }

    /// Check whether the given snapshot has been saved to disk.
    fn is_snapshot_saved(&self, id: SnapshotId) -> bool {
        self.saved_snapshot == Some(id)
    }

    /// Mark the view as saved at a given snapshot.
    fn saved(&mut self, id: SnapshotId, storage: FileStorage) {
        self.file_status = FileStatus::Saved(storage);
        self.saved_snapshot = Some(id);
    }

    /// Reset the view by providing frame size and number of frames.
    fn reset(&mut self, extent: ViewExtent) {
        self.fw = extent.fw;
        self.fh = extent.fh;

        let mut frames = Vec::new();
        let origin = Rect::origin(self.fw as f32, self.fh as f32);

        for i in 0..extent.nframes {
            frames.push(origin + Vector2::new(i as f32 * self.fw as f32, 0.));
        }
        self.animation = Animation::new(&frames, self.animation.delay);
    }
}

///////////////////////////////////////////////////////////////////////////////

/// Status of the underlying file displayed by the view.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum FileStatus {
    /// There is no file being displayed.
    NoFile,
    /// The file is new and unsaved.
    New(FileStorage),
    /// The file is saved and unmodified.
    Saved(FileStorage),
    /// The file has been modified since the last save.
    Modified(FileStorage),
}

impl ToString for FileStatus {
    fn to_string(&self) -> String {
        match self {
            FileStatus::NoFile => String::new(),
            FileStatus::Saved(ref storage) => format!("{}", storage),
            FileStatus::New(ref storage) => format!("{} [new]", storage),
            FileStatus::Modified(ref storage) => format!("{} [modified]", storage),
        }
    }
}

/// Representation of the view data on disk.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum FileStorage {
    /// Stored as a range of files.
    Range(NonEmpty<PathBuf>),
    /// Stored as a single file.
    Single(PathBuf),
}

impl fmt::Display for FileStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Range(paths) => {
                let parent = paths.first().parent();

                if paths.iter().all(|p| p.parent() == parent) {
                    let first = paths
                        .first()
                        .file_stem()
                        .expect("the path has a file stem")
                        .to_string_lossy();
                    let last = paths
                        .last()
                        .file_name()
                        .expect("the path has a file name")
                        .to_string_lossy();

                    if let Some(parent) = parent {
                        write!(f, "{}/{} .. {}", parent.display(), first, last)
                    } else {
                        write!(f, "{} .. {}", first, last)
                    }
                } else {
                    write!(f, "*")
                }
            }
            Self::Single(path) => write!(f, "{}", path.display()),
        }
    }
}

impl From<&Path> for FileStorage {
    fn from(p: &Path) -> Self {
        FileStorage::Single(p.into())
    }
}

impl From<PathBuf> for FileStorage {
    fn from(p: PathBuf) -> Self {
        FileStorage::Single(p)
    }
}

impl FileStorage {
    pub fn contains<P: AsRef<Path>>(&self, p: P) -> bool {
        match self {
            Self::Single(buf) => buf.as_path() == p.as_ref(),
            Self::Range(bufs) => bufs.iter().any(|buf| buf.as_path() == p.as_ref()),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////

/// Manages views.
#[derive(Debug)]
pub struct ViewManager {
    /// Currently active view.
    pub active_id: ViewId,

    /// View dictionary.
    views: BTreeMap<ViewId, View>,

    /// The next `ViewId`.
    next_id: ViewId,

    /// A last-recently-used list of views.
    lru: VecDeque<ViewId>,
}

impl ViewManager {
    /// Maximum number of views in the view LRU list.
    const MAX_LRU: usize = Session::MAX_VIEWS;

    /// New empty view manager.
    pub fn new() -> Self {
        Self {
            active_id: ViewId::default(),
            next_id: ViewId(1),
            views: BTreeMap::new(),
            lru: VecDeque::new(),
        }
    }

    /// Add a view.
    pub fn add(&mut self, fs: FileStatus, w: u32, h: u32, nframes: usize, delay: u64) -> ViewId {
        let id = self.gen_id();
        let view = View::new(id, fs, w, h, nframes, delay);

        self.views.insert(id, view);

        id
    }

    /// Remove a view.
    pub fn remove(&mut self, id: ViewId) {
        self.views.remove(&id);
        self.lru.retain(|v| *v != id);

        self.active_id = self
            .recent()
            .or_else(|| self.last().map(|v| v.id))
            .unwrap_or(ViewId::default());
    }

    /// Return the id of the last recently active view, if any.
    pub fn recent(&self) -> Option<ViewId> {
        self.lru.front().cloned()
    }

    /// Return the currently active view, if any.
    pub fn active(&self) -> Option<&View> {
        self.views.get(&self.active_id)
    }

    /// Return the currently active view mutably, if any.
    pub fn active_mut(&mut self) -> Option<&mut View> {
        self.views.get_mut(&self.active_id)
    }

    /// Activate a view.
    pub fn activate(&mut self, id: ViewId) {
        debug_assert!(
            self.views.contains_key(&id),
            "the view being activated exists"
        );
        if self.active_id == id {
            return;
        }
        self.active_id = id;
        self.lru.push_front(id);
        self.lru.truncate(Self::MAX_LRU);
    }

    /// Iterate over views.
    pub fn iter(&self) -> btree_map::Values<'_, ViewId, View> {
        self.views.values()
    }

    /// Iterate over views, mutably.
    pub fn iter_mut(&mut self) -> btree_map::ValuesMut<'_, ViewId, View> {
        self.views.values_mut()
    }

    /// Get a view, mutably.
    pub fn get(&self, id: ViewId) -> Option<&View> {
        self.views.get(&id)
    }

    /// Get a view, mutably.
    pub fn get_mut(&mut self, id: ViewId) -> Option<&mut View> {
        self.views.get_mut(&id)
    }

    /// Find a view.
    pub fn find<F>(&self, f: F) -> Option<&View>
    where
        for<'r> F: Fn(&'r &View) -> bool,
    {
        self.iter().find(f)
    }

    /// Iterate over view ids.
    pub fn ids<'a>(&'a self) -> impl DoubleEndedIterator<Item = ViewId> + 'a {
        self.views.keys().cloned()
    }

    /// Get `ViewId` *after* given id.
    pub fn after(&self, id: ViewId) -> Option<ViewId> {
        self.range(id..).nth(1)
    }

    /// Get `ViewId` *before* given id.
    pub fn before(&self, id: ViewId) -> Option<ViewId> {
        self.range(..id).next_back()
    }

    /// Get the first view.
    pub fn first(&self) -> Option<&View> {
        self.iter().next()
    }

    /// Get the first view, mutably.
    pub fn first_mut(&mut self) -> Option<&mut View> {
        self.iter_mut().next()
    }

    /// Get the last view.
    pub fn last(&self) -> Option<&View> {
        self.iter().next_back()
    }

    /// Get view id range.
    pub fn range<'a, R>(&'a self, r: R) -> impl DoubleEndedIterator<Item = ViewId> + 'a
    where
        R: std::ops::RangeBounds<ViewId>,
    {
        self.views.range(r).map(|(id, _)| *id)
    }

    /// Whether there are views.
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    /// Generate a new view id.
    fn gen_id(&mut self) -> ViewId {
        let ViewId(id) = self.next_id;
        self.next_id = ViewId(id + 1);

        ViewId(id)
    }
}
