//! Where you left off.
//!
//! One line per film, in a file of its own, most recently watched first. Small
//! enough to read on the way to the first frame and to rewrite whenever a film
//! is left, which is what makes it survive the application being killed rather
//! than closed — a television box is switched off at the wall, not quit.
//!
//! **Two rules decide what is worth keeping**, and between them they are the
//! whole of the design:
//!
//! * A film barely started is not a film you left. Somebody who opened
//!   something, watched a minute and went elsewhere did not ask to be put back
//!   there, and being offered the minute they had already seen is worse than
//!   being offered nothing.
//! * A film nearly finished is finished. The credits are not a place to
//!   resume; landing there on the next open is the one behaviour that makes
//!   people turn the feature off. So reaching the end **forgets** the film
//!   rather than remembering the end of it.
//!
//! The file's modification time is kept beside the position, so a film
//! replaced by another of the same name — a better copy of the same episode —
//! starts at its beginning rather than an hour into something else.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// How far in a film has to be before it is worth remembering.
const NOT_YET: f64 = 60.0;

/// How much has to be left. Less than this and the film is finished.
const AS_GOOD_AS_DONE: f64 = 90.0;

/// How many films are remembered. Beyond this the least recently watched are
/// dropped, which is what a list somebody never asked to curate should do.
const HOW_MANY: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Left {
    at: f64,
    /// When the film itself was last written, so a file replaced under the
    /// same name is not resumed into.
    stamp: u64,
}

#[derive(Debug, Default)]
pub struct Resume {
    /// Most recently watched first, which is both the order it is written in
    /// and what decides who is dropped when the list is full.
    order: Vec<PathBuf>,
    marks: HashMap<PathBuf, Left>,
    /// Whether anything has changed since it was read or written. Kept so that
    /// leaving a film nobody had watched a minute of does not rewrite the file.
    changed: bool,
}

impl Resume {
    /// Read the record, or start an empty one. Never fails: a machine with no
    /// state directory is a machine that remembers nothing, which is a
    /// perfectly good way for this to work.
    pub fn load() -> Resume {
        let mut resume = Resume::default();
        let Some(path) = record() else {
            return resume;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return resume;
        };
        for line in text.lines() {
            let mut fields = line.splitn(3, '\t');
            let (Some(at), Some(stamp), Some(film)) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Ok(at), Ok(stamp)) = (at.parse::<f64>(), stamp.parse::<u64>()) else {
                continue;
            };
            if !at.is_finite() || at <= 0.0 || film.is_empty() {
                continue;
            }
            let film = PathBuf::from(film);
            if resume.marks.contains_key(&film) {
                continue;
            }
            resume.order.push(film.clone());
            resume.marks.insert(film, Left { at, stamp });
        }
        resume
    }

    /// Where this film was left, if it is worth going back to.
    ///
    /// `None` for a film that was never watched, one that was finished, and
    /// one whose file has been written since — all three of which mean "start
    /// at the beginning" and none of which is worth telling the caller apart.
    pub fn at(&self, film: &Path) -> Option<f64> {
        let left = self.marks.get(film)?;
        if written(film) != left.stamp {
            return None;
        }
        Some(left.at)
    }

    /// Say where a film was left, or that it is finished.
    ///
    /// Called when a film is closed, stepped away from, or reaches its end —
    /// all of which are the same act as far as this is concerned. The two
    /// rules at the top of this file are applied here, so no caller has to
    /// know them.
    pub fn note(&mut self, film: &Path, at: f64, length: f64) {
        let worth_it =
            at.is_finite() && at > NOT_YET && (length <= 0.0 || length - at > AS_GOOD_AS_DONE);
        if !worth_it {
            self.forget(film);
            return;
        }
        let mark = Left {
            at,
            stamp: written(film),
        };
        if self.marks.insert(film.to_path_buf(), mark) != Some(mark) {
            self.changed = true;
        }
        self.order.retain(|known| known != film);
        self.order.insert(0, film.to_path_buf());
        if self.order.len() > HOW_MANY {
            for dropped in self.order.drain(HOW_MANY..) {
                self.marks.remove(&dropped);
            }
        }
    }

    pub fn forget(&mut self, film: &Path) {
        if self.marks.remove(film).is_some() {
            self.changed = true;
        }
        self.order.retain(|known| known != film);
    }

    /// Write the record, if there is anything new in it.
    ///
    /// Written whole and renamed into place rather than appended to: the file
    /// is a few tens of kilobytes at its largest, and a half-written record is
    /// a film resumed into the middle of another film's path.
    pub fn save(&mut self) {
        if !self.changed {
            return;
        }
        let Some(path) = record() else {
            return;
        };
        let Some(folder) = path.parent() else {
            return;
        };
        if std::fs::create_dir_all(folder).is_err() {
            return;
        }
        let mut text = String::new();
        for film in &self.order {
            let Some(left) = self.marks.get(film) else {
                continue;
            };
            let Some(name) = film.to_str() else {
                // A path that is not text cannot be written to a line-based
                // record, and a film nobody can name is a film nobody can
                // resume. It is dropped rather than mangled.
                continue;
            };
            text.push_str(&format!("{:.1}\t{}\t{name}\n", left.at, left.stamp));
        }
        let temporary = folder.join(format!(".resume-{}", std::process::id()));
        if std::fs::write(&temporary, text).is_err() {
            let _ = std::fs::remove_file(&temporary);
            return;
        }
        if std::fs::rename(&temporary, &path).is_err() {
            let _ = std::fs::remove_file(&temporary);
            return;
        }
        self.changed = false;
    }
}

fn written(film: &Path) -> u64 {
    std::fs::metadata(film)
        .ok()
        .and_then(|about| about.modified().ok())
        .and_then(|when| when.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Where the record lives.
///
/// The state directory rather than the config one, because this is not
/// something anybody configures — it is what the application noticed. A
/// machine that names neither keeps nothing.
fn record() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("VIDEONSOLE_STATE_DIR") {
        return Some(PathBuf::from(named).join("resume"));
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| crate::library::home().map(|home| home.join(".local/state")))?;
    Some(state.join("videonsole").join("resume"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A film barely started is not a film you left.
    #[test]
    fn the_first_minute_is_not_worth_remembering() {
        let mut resume = Resume::default();
        resume.note(Path::new("/f/a.mkv"), 12.0, 6000.0);
        assert_eq!(resume.at(Path::new("/f/a.mkv")), None);
    }

    /// And a film nearly over is over. Landing on the credits is the one
    /// behaviour that makes people turn this off.
    #[test]
    fn the_end_of_a_film_is_forgotten_rather_than_remembered() {
        let mut resume = Resume::default();
        resume.note(Path::new("/f/a.mkv"), 3000.0, 6000.0);
        assert!(resume.at(Path::new("/f/a.mkv")).is_some());
        resume.note(Path::new("/f/a.mkv"), 5980.0, 6000.0);
        assert_eq!(
            resume.at(Path::new("/f/a.mkv")),
            None,
            "reaching the end forgets the film"
        );
    }

    /// A film whose length the container never declared is still worth
    /// remembering — there is no end to be near.
    #[test]
    fn a_film_of_unknown_length_is_remembered_anyway() {
        let mut resume = Resume::default();
        resume.note(Path::new("/f/a.mkv"), 300.0, 0.0);
        assert_eq!(resume.at(Path::new("/f/a.mkv")), Some(300.0));
    }

    /// The file this remembers is a file on a disk, and a different file of
    /// the same name is a different film.
    #[test]
    fn a_replaced_file_starts_at_its_beginning() {
        let mut resume = Resume::default();
        // Nothing at this path, so `written` answers nought both times and the
        // mark stands.
        resume.note(Path::new("/no/such/film.mkv"), 300.0, 6000.0);
        assert_eq!(resume.at(Path::new("/no/such/film.mkv")), Some(300.0));
        // A mark whose stamp does not match the file is not offered.
        resume.marks.insert(
            PathBuf::from("/no/such/film.mkv"),
            Left {
                at: 300.0,
                stamp: 12345,
            },
        );
        assert_eq!(resume.at(Path::new("/no/such/film.mkv")), None);
    }

    #[test]
    fn the_list_is_most_recently_watched_first_and_has_an_end() {
        let mut resume = Resume::default();
        for number in 0..HOW_MANY + 20 {
            resume.note(&PathBuf::from(format!("/f/{number}.mkv")), 300.0, 6000.0);
        }
        assert_eq!(resume.order.len(), HOW_MANY);
        assert_eq!(resume.marks.len(), HOW_MANY);
        assert_eq!(
            resume.order[0],
            PathBuf::from(format!("/f/{}.mkv", HOW_MANY + 19)),
            "the newest is first"
        );
        assert_eq!(
            resume.at(Path::new("/f/0.mkv")),
            None,
            "the oldest fell off the end"
        );
    }

    #[test]
    fn nothing_new_is_nothing_to_write() {
        let mut resume = Resume::default();
        assert!(!resume.changed);
        resume.note(Path::new("/f/a.mkv"), 10.0, 6000.0);
        assert!(
            !resume.changed,
            "a film not worth remembering changed nothing"
        );
        resume.note(Path::new("/f/a.mkv"), 300.0, 6000.0);
        assert!(resume.changed);
    }
}
