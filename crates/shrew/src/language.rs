//! *Set Language…* (the status bar's language, palette *View: Set Language…*, `:setf [name]`): the active buffer's
//! language picked by hand. It sets the grammar that highlights it and the comment tokens Ctrl+/ uses, and stays
//! with the buffer (`Document::language`) until it is set back to *Auto Detect*.

use crate::state::State;
use shrew_core::Choice;
use shrew_core::picker::{PickerData, PickerItem};

/// The list's value for "detect it again".
const AUTO: &str = "";

/// Opens the list: *Auto Detect*, then every grammar (or, without highlighting, shrew's own languages), the current
/// one selected.
pub fn open(st: &mut State) {
    let item = |label: String, detail: &str, value: &str| PickerItem {
        label,
        detail: detail.to_string(),
        data: PickerData::Value(value.to_string()),
    };
    let current = st.ed.buf().doc.language.clone();
    let shown = st.language();
    let auto = match (&current, &shown) {
        (None, Some(l)) => format!("now {l}"),
        _ => "from the file name".to_string(),
    };
    let mut items = vec![item("Auto Detect".into(), &auto, AUTO)];
    let names = grammar_names(st);
    let now = current.as_deref().or(shown.as_deref());
    for n in &names {
        let detail = if Some(n.as_str()) == now { "current" } else { "" };
        items.push(item(n.clone(), detail, n));
    }
    let selected = match &current {
        None => 0,
        Some(c) => items.iter().position(|i| matches!(&i.data, PickerData::Value(v) if v == c)).unwrap_or(0),
    };
    st.ed.open_choice(Choice::Language, items, selected);
}

/// The grammars' names when they are loaded, else shrew's languages.
fn grammar_names(st: &State) -> Vec<String> {
    #[cfg(feature = "syntax")]
    if let Some(s) = &st.syntax {
        let names = s.syntax_names();
        if !names.is_empty() {
            return names;
        }
    }
    let _ = st;
    shrew_core::lang::LANGS.iter().map(|l| l.name.to_string()).collect()
}

/// A language picked (Enter in the list, `:setf rust`); [`AUTO`] goes back to detecting it.
pub fn choose(st: &mut State, value: String) {
    let value = value.trim().to_string();
    let b = st.ed.buf_mut();
    b.doc.language = (value != AUTO).then(|| value.clone());
    let id = b.id;
    apply(st, id);
    if value == AUTO {
        st.ed.info("language detected from the file again");
    } else {
        st.ed.info(format!("language {value}"));
    }
}

/// Gives buffer `id`'s highlighter the picked language (or detection back). Also after a highlighter is made anew.
pub fn apply(st: &mut State, id: u64) {
    #[cfg(feature = "syntax")]
    {
        let Some(b) = st.ed.buffers.iter().find(|b| b.id == id) else { return };
        let Some(hl) = st.highlighters.get_mut(&id) else { return };
        match &b.doc.language {
            Some(l) => hl.set_language(l),
            None => hl.detect_language(b.doc.path.as_deref(), &b.doc.rope),
        }
    }
    #[cfg(not(feature = "syntax"))]
    let _ = (st, id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use shrew_core::Document;
    use std::sync::Arc;

    fn state(text: &str) -> State {
        let mut st = State::new(shrew_config::Loaded::default(), Arc::new(|| {}), true);
        st.ed.install_document(Document::from_str(text));
        st.process_events();
        st
    }

    #[test]
    fn a_picked_language_sets_the_comment_tokens_and_goes_back_to_auto() {
        let mut st = state("x = 1\n");
        assert!(st.ed.lang().is_none(), "a scratch buffer has no language");
        st.ed.run(shrew_core::Command::SetLanguage);
        st.process_events();
        let p = st.ed.picker.as_ref().expect("the list opens");
        assert_eq!(p.kind, shrew_core::picker::PickerKind::Choice(Choice::Language));
        assert_eq!(p.items[0].label, "Auto Detect");
        assert!(p.items.len() > 10, "every language is there");
        // type the name, Enter
        for k in shrew_core::KeyPress::parse_seq("python<CR>") {
            st.key(k);
        }
        assert!(st.ed.picker.is_none());
        let picked = st.ed.buf().doc.language.clone().expect("picked");
        assert!(picked.eq_ignore_ascii_case("python"), "{picked}");
        assert_eq!(st.ed.lang().map(|l| l.name), Some("python"));
        assert_eq!(st.ed.lang().and_then(|l| l.line_comment), Some("#"));
        // :setf sets it from the command line; an empty value is Auto Detect
        st.ed.run_ex("setf rust");
        st.process_events();
        assert_eq!(st.ed.lang().map(|l| l.name), Some("rust"));
        choose(&mut st, String::new());
        assert_eq!(st.ed.buf().doc.language, None);
        assert!(st.ed.lang().is_none());
    }

    #[test]
    fn grammar_names_map_to_languages() {
        use shrew_core::lang::by_grammar;
        for (grammar, lang) in
            [("Rust", "rust"), ("C++", "cpp"), ("Bourne Again Shell (bash)", "bash"), ("Python", "python")]
        {
            assert_eq!(by_grammar(grammar).map(|l| l.name), Some(lang), "{grammar}");
        }
        assert!(by_grammar("Nothing Like It").is_none());
    }
}
