//! Which Sublime/TextMate scopes get which syntax role. shrew-syntax turns these into syntect theme rules; the
//! importers run them backwards (which color does a foreign scheme give each role's typical scope).
//!
//! Selectors follow Sublime's rules: the most specific match wins, so `keyword.operator.word` (a keyword) beats
//! `keyword.operator` (an operator).

use crate::tokens::{Font, Role};

const B: Font = Font::BOLD;
const I: Font = Font::ITALIC;
const U: Font = Font::UNDERLINE;
const P: Font = Font::PLAIN;

/// (scope selector, role, the role's usual font style there).
pub static SCOPE_RULES: &[(&str, Role, Font)] = &[
    ("comment, punctuation.definition.comment", Role::Comment, I),
    (
        "comment.block.documentation, comment.line.documentation, comment.block.documentation punctuation.definition.comment, comment.line.documentation punctuation.definition.comment",
        Role::DocComment,
        I,
    ),
    ("string, punctuation.definition.string, meta.string", Role::String, P),
    ("constant.character.escape, constant.other.placeholder, constant.character.format", Role::StringEscape, P),
    ("string.regexp", Role::Regex, P),
    ("constant.numeric", Role::Number, P),
    (
        "constant.language, constant.character, constant.other, support.constant, variable.other.constant, entity.name.constant, entity.name.variable.constant",
        Role::Constant,
        P,
    ),
    ("keyword, storage, keyword.operator.word, keyword.operator.logical.python", Role::Keyword, P),
    (
        "keyword.control, keyword.control.flow, keyword.control.conditional, keyword.control.loop, keyword.control.return, keyword.control.exception",
        Role::Control,
        P,
    ),
    (
        "storage.type.primitive, storage.type.numeric, storage.type.builtin, support.type.primitive, support.type.builtin, storage.type.boolean, storage.type.string",
        Role::BuiltinType,
        P,
    ),
    ("keyword.operator, keyword.operator.assignment, keyword.operator.arithmetic", Role::Operator, P),
    ("punctuation, meta.brace", Role::Punctuation, P),
    ("variable.language, variable.parameter.function.language.special", Role::Constant, I),
    ("variable.parameter", Role::Parameter, P),
    ("variable, variable.other.readwrite", Role::Variable, P),
    (
        "variable.other.member, variable.other.property, variable.other.object.property, support.type.property-name, meta.object-literal.key, entity.name.tag.yaml, string.unquoted.key, entity.name.key, meta.mapping.key string, meta.structure.dictionary.key string, meta.toml.key, entity.name.tag.toml",
        Role::Property,
        P,
    ),
    ("entity.name.function, support.function, variable.function, meta.function-call.identifier", Role::Function, P),
    (
        "entity.name.function.method, entity.name.method, variable.function.method, support.function.method, meta.method-call variable.function, meta.function-call.method variable.function",
        Role::Method,
        P,
    ),
    (
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.union, entity.name.trait, entity.name.interface, entity.name.impl, support.type, support.class, entity.other.inherited-class, meta.generic",
        Role::Type,
        P,
    ),
    (
        "storage.type.function, storage.type.struct, storage.type.enum, storage.type.trait, storage.type.impl, storage.type.module, storage.type.class, storage.type.type, storage.type.union",
        Role::Keyword,
        P,
    ),
    (
        "entity.name.namespace, entity.name.module, support.module, meta.path, support.other.namespace, entity.name.type.namespace, entity.name.type.module",
        Role::Namespace,
        P,
    ),
    (
        "entity.name.macro, support.macro, keyword.directive, meta.preprocessor, keyword.control.import.c, keyword.control.include, support.function.macro",
        Role::Macro,
        P,
    ),
    (
        "meta.annotation, meta.attribute, meta.decorator, entity.name.function.decorator, punctuation.definition.annotation, punctuation.definition.decorator, entity.other.attribute-name",
        Role::Attribute,
        P,
    ),
    (
        "storage.modifier.lifetime, entity.name.lifetime, entity.name.label, punctuation.definition.lifetime",
        Role::Macro,
        I,
    ),
    ("entity.name.tag, punctuation.definition.tag", Role::Tag, P),
    ("invalid, invalid.illegal", Role::Invalid, U),
    ("invalid.deprecated", Role::Invalid, I),
    // markup
    ("markup.heading, markup.heading entity.name.section, entity.name.section", Role::Heading, B),
    ("markup.bold", Role::Emphasis, B),
    ("markup.italic", Role::Emphasis, I),
    ("markup.underline", Role::Emphasis, U),
    ("markup.underline.link, markup.link, meta.link string.other.link, string.other.link", Role::Link, U),
    ("markup.raw, markup.inline.raw, markup.raw.block", Role::String, P),
    ("markup.quote", Role::Comment, I),
    ("markup.list punctuation.definition.list_item, punctuation.definition.list_item", Role::Keyword, P),
    // diffs
    ("markup.inserted, meta.diff.header.to-file, punctuation.definition.inserted", Role::DiffAdded, P),
    ("markup.deleted, meta.diff.header.from-file, punctuation.definition.deleted", Role::DiffRemoved, P),
    ("markup.changed, punctuation.definition.changed", Role::DiffChanged, P),
    ("meta.diff.range, meta.diff.index, meta.separator.diff", Role::Type, I),
];

/// A typical scope stack for each role, for reading a foreign color scheme back into roles (most specific last).
pub(crate) fn sample_scopes(r: Role) -> &'static [&'static str] {
    match r {
        Role::Keyword => &["source.rust", "keyword.other.rust"],
        Role::Control => &["source.rust", "keyword.control.rust"],
        Role::String => &["source.rust", "string.quoted.double.rust"],
        Role::StringEscape => &["source.rust", "string.quoted.double.rust", "constant.character.escape.rust"],
        Role::Number => &["source.rust", "constant.numeric.integer.rust"],
        Role::Constant => &["source.rust", "constant.language.rust"],
        Role::Comment => &["source.rust", "comment.line.double-slash.rust"],
        Role::DocComment => &["source.rust", "comment.line.documentation.rust"],
        Role::Function => &["source.rust", "meta.function.rust", "entity.name.function.rust"],
        Role::Method => &["source.js", "meta.function-call.method.js", "variable.function.js"],
        Role::Type => &["source.rust", "meta.struct.rust", "entity.name.struct.rust"],
        Role::BuiltinType => &["source.rust", "storage.type.primitive.rust"],
        Role::Property => &["source.js", "meta.object-literal.key.js"],
        Role::Variable => &["source.js", "variable.other.readwrite.js"],
        Role::Parameter => &["source.rust", "meta.function.parameters.rust", "variable.parameter.rust"],
        Role::Namespace => &["source.rust", "meta.path.rust", "entity.name.namespace.rust"],
        Role::Macro => &["source.rust", "support.macro.rust"],
        Role::Attribute => &["source.rust", "meta.annotation.rust"],
        Role::Tag => &["text.html.basic", "meta.tag.block.any.html", "entity.name.tag.block.any.html"],
        Role::Operator => &["source.rust", "keyword.operator.arithmetic.rust"],
        Role::Punctuation => &["source.rust", "punctuation.separator.rust"],
        Role::Regex => &["source.js", "string.regexp.js"],
        Role::Heading => &["text.html.markdown", "markup.heading.1.markdown"],
        Role::Link => &["text.html.markdown", "meta.link.inline.markdown", "markup.underline.link.markdown"],
        Role::Emphasis => &["text.html.markdown", "markup.italic.markdown"],
        Role::DiffAdded => &["source.diff", "markup.inserted.diff"],
        Role::DiffRemoved => &["source.diff", "markup.deleted.diff"],
        Role::DiffChanged => &["source.diff", "markup.changed.diff"],
        Role::Invalid => &["source.rust", "invalid.illegal.rust"],
    }
}
