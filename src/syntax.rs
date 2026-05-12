use syntect::parsing::SyntaxSet;
use crate::editor::Editor;

pub trait SyntaxExt {
    fn init_syntax() -> SyntaxSet;
    fn clear_cache(&mut self);
    fn mark_modified(&mut self);
}

impl SyntaxExt for Editor {
    fn init_syntax() -> SyntaxSet {
        SyntaxSet::load_defaults_newlines()
    }

    fn clear_cache(&mut self) {
        self.highlight_cache.clear();
    }

    fn mark_modified(&mut self) {
        self.is_modified = true;
        self.clear_cache();
    }
}