//! LISP loaded at startup, from CADCraft's support folder (the app sets it: `<settings>/support`):
//!
//! - `acad.lsp` once per session, in the first drawing (every drawing with ACADLSPASDOC on);
//! - `acaddoc.lsp` in every drawing opened or created;
//! - then `S::STARTUP`, when they define it.
//!
//! Never from a drawing's own folder: an `acaddoc.lsp` that came along with a downloaded drawing
//! would run on opening it, the way LISP viruses spread.

use std::path::PathBuf;

use super::value::Value;
use crate::Session;

/// The startup files, in load order: (name, once per session unless ACADLSPASDOC).
const FILES: [(&str, bool); 2] = [("acad.lsp", true), ("acaddoc.lsp", false)];

impl Session {
    /// The folder startup LISP files are read from (`None`: no startup files, the default; the
    /// app sets it).
    pub fn set_lisp_support_dir(&mut self, dir: Option<PathBuf>) {
        self.lisp.support_dir = dir;
    }

    /// The folder startup LISP files are read from.
    pub fn lisp_support_dir(&self) -> Option<&std::path::Path> {
        self.lisp.support_dir.as_deref()
    }

    /// Load the startup files into the active drawing, once (from [`Session::lisp_tick`]).
    pub(crate) fn lisp_autoload(&mut self) {
        if self.lisp.active {
            return;
        }
        let Ok(uid) = self.state().map(|s| s.uid) else { return };
        if self.lisp.autoloaded.insert(uid) {
            if let Some(dir) = self.lisp.support_dir.clone() {
                let per_drawing = self.settings.acadlspasdoc;
                for (name, once) in FILES {
                    if once && self.lisp.acad_loaded && !per_drawing {
                        continue;
                    }
                    let Some(text) = super::builtins::read_text_file(&dir.join(name).to_string_lossy()) else { continue };
                    if once {
                        self.lisp.acad_loaded = true;
                    }
                    self.lisp.files.insert(name.trim_end_matches(".lsp").to_string(), text.clone());
                    // Startup files don't ask for input: they run to the end. Errors are shown.
                    if let Err(e) = self.lisp_eval(&text, false, true, Vec::new()) {
                        self.echo(format!("{name}: {e}"));
                    }
                }
            }
            if matches!(self.lisp_var("S::STARTUP"), Value::Func(_) | Value::Subr(_)) {
                let _ = self.lisp_eval("(S::STARTUP)", false, true, Vec::new());
            }
        }

        // Clone the bounded catalog so evaluating an entry point can mutate the session without
        // holding a borrow into it. A failed entry point is recorded as attempted and is not run
        // on every frame; replacing the catalog lets the user retry after updating it.
        let pending: Vec<(String, String, String)> = self
            .extensions()
            .iter()
            .filter(|extension| extension.enabled && !self.lisp.extension_loaded.contains(&(uid, extension.manifest.id.clone())))
            .map(|extension| (extension.manifest.id.clone(), extension.manifest.entrypoint.clone(), extension.entrypoint.clone()))
            .collect();
        for (id, entrypoint, source) in pending {
            self.lisp.extension_loaded.insert((uid, id.clone()));
            let forms = match super::read::read_all(&source) {
                Ok(forms) => forms,
                Err(error) => {
                    self.echo(format!("{id}/{entrypoint}: ; error: {}", error.msg));
                    continue;
                }
            };
            let result = self.lisp_start(super::eval::Machine::forms(forms), false, true, Vec::new(), format!("Load {id}"), Some(id.clone()));
            match result {
                Err(error) => self.echo(format!("{id}/{entrypoint}: {error}")),
                Ok(Some(Err(error))) => self.echo(format!("{id}/{entrypoint}: {error}")),
                Ok(_) => self.echo(format!("{id} successfully loaded.")),
            }
        }
    }
}
