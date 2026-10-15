#!/usr/bin/env bash
# Builds a small, deterministic git repository for the git screenshots (scripts/screenshots.sh): a toy expression
# parser with a few authors, a merged and an unmerged branch, a tag, an upstream, and a working tree with
# unstaged, staged and untracked changes. Commit dates are "N days ago" from now, so relative ages ("3 days ago")
# read the same whenever it runs.
#
#   scripts/demo-repo.sh DIR       # DIR must not exist yet
set -euo pipefail

dir=${1:?usage: demo-repo.sh DIR}
[ ! -e "$dir" ] || { echo "$dir exists" >&2; exit 1; }
mkdir -p "$dir"
dir=$(cd "$dir" && pwd)
cd "$dir"

export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
git init -q -b main
git config commit.gpgsign false
git config core.autocrlf false
git config user.name "Ann Rivera"
git config user.email ann@example.com

# commit AUTHOR DAYS_AGO MESSAGE  (stages everything)
commit() {
  local who=$1 days=$2 msg=$3 when
  when=$(date -d "-$days days" +%Y-%m-%dT%H:%M:%S%z)
  git add -A
  case $who in
    ann) name="Ann Rivera" mail=ann@example.com ;;
    ben) name="Ben Okafor" mail=ben@example.com ;;
    chidi) name="Chidi Eze" mail=chidi@example.com ;;
  esac
  GIT_AUTHOR_NAME=$name GIT_AUTHOR_EMAIL=$mail GIT_COMMITTER_NAME=$name GIT_COMMITTER_EMAIL=$mail \
    GIT_AUTHOR_DATE=$when GIT_COMMITTER_DATE=$when git commit -q -m "$msg"
}
merge() {
  local days=$1 branch=$2 when
  when=$(date -d "-$days days" +%Y-%m-%dT%H:%M:%S%z)
  GIT_AUTHOR_NAME="Ann Rivera" GIT_AUTHOR_EMAIL=ann@example.com GIT_COMMITTER_NAME="Ann Rivera" \
    GIT_COMMITTER_EMAIL=ann@example.com GIT_AUTHOR_DATE=$when GIT_COMMITTER_DATE=$when \
    git merge -q --no-ff -m "Merge branch '$branch'" "$branch"
}

mkdir -p src tests
cat >Cargo.toml <<'EOF'
[package]
name = "calc"
version = "0.1.0"
edition = "2024"

[dependencies]
EOF
cat >README.md <<'EOF'
# calc

A tiny expression calculator: a lexer, a precedence-climbing parser and an evaluator.
EOF
cat >src/lib.rs <<'EOF'
//! calc: lex, parse and evaluate arithmetic.

pub mod lexer;
pub mod parser;
EOF
cat >src/lexer.rs <<'EOF'
//! Turns text into tokens.

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '0'..='9' => {
                let mut text = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() {
                        text.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Number(text.parse().map_err(|_| "bad number")?));
            }
            '+' => {
                tokens.push(Token::Plus);
                chars.next();
            }
            '-' => {
                tokens.push(Token::Minus);
                chars.next();
            }
            '*' => {
                tokens.push(Token::Star);
                chars.next();
            }
            '/' => {
                tokens.push(Token::Slash);
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            other => return Err(format!("unexpected {other}")),
        }
    }
    Ok(tokens)
}
EOF
commit ann 40 "Start the expression calculator"

cat >src/parser.rs <<'EOF'
//! Builds an expression tree from tokens.

use crate::lexer::Token;

#[derive(Debug, PartialEq)]
pub enum Expr {
    Number(f64),
    Neg(Box<Expr>),
    Binary(Box<Expr>, char, Box<Expr>),
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Parser {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    pub fn parse(&mut self) -> Result<Expr, String> {
        let e = self.expr()?;
        match self.peek() {
            None => Ok(e),
            Some(t) => Err(format!("unexpected {t:?}")),
        }
    }

    fn expr(&mut self) -> Result<Expr, String> {
        let mut left = self.term()?;
        while let Some(op) = self.peek() {
            let c = match op {
                Token::Plus => '+',
                Token::Minus => '-',
                _ => break,
            };
            self.next();
            let right = self.term()?;
            left = Expr::Binary(Box::new(left), c, Box::new(right));
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<Expr, String> {
        let mut left = self.factor()?;
        while let Some(op) = self.peek() {
            let c = match op {
                Token::Star => '*',
                Token::Slash => '/',
                _ => break,
            };
            self.next();
            let right = self.factor()?;
            left = Expr::Binary(Box::new(left), c, Box::new(right));
        }
        Ok(left)
    }

    fn factor(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Token::Number(n)) => Ok(Expr::Number(n)),
            Some(Token::Minus) => Ok(Expr::Neg(Box::new(self.factor()?))),
            Some(Token::LParen) => {
                let e = self.expr()?;
                match self.next() {
                    Some(Token::RParen) => Ok(e),
                    _ => Err("expected )".into()),
                }
            }
            other => Err(format!("unexpected {other:?}")),
        }
    }
}
EOF
commit ben 34 "Add the parser"

sed -i 's/                    if d.is_ascii_digit() {/                    if d.is_ascii_digit() || d == '"'"'.'"'"' || d == '"'"'_'"'"' {/' src/lexer.rs
sed -i 's/                        text.push(d);/                        if d != '"'"'_'"'"' {\n                            text.push(d);\n                        }/' src/lexer.rs
commit ann 28 "Lexer: floats and digit separators"

git checkout -q -b feature/eval
cat >src/eval.rs <<'EOF'
//! Evaluates an expression tree.

use crate::parser::Expr;

pub fn eval(e: &Expr) -> Result<f64, String> {
    Ok(match e {
        Expr::Number(n) => *n,
        Expr::Neg(inner) => -eval(inner)?,
        Expr::Binary(l, op, r) => {
            let (a, b) = (eval(l)?, eval(r)?);
            match op {
                '+' => a + b,
                '-' => a - b,
                '*' => a * b,
                '/' => a / b,
                _ => return Err(format!("unknown operator {op}")),
            }
        }
    })
}
EOF
echo "pub mod eval;" >>src/lib.rs
commit ben 21 "Add an evaluator"
sed -i "s|                '/' => a / b,|                '/' if b == 0.0 => return Err(\"division by zero\".into()),\n                '/' => a / b,|" src/eval.rs
commit ben 19 "Evaluator: report division by zero"

git checkout -q main
cat >tests/lexer.rs <<'EOF'
use calc::lexer::{Token, tokenize};

#[test]
fn numbers_and_operators() {
    assert_eq!(tokenize("1 + 2").unwrap(), [Token::Number(1.0), Token::Plus, Token::Number(2.0)]);
}

#[test]
fn rejects_unknown_characters() {
    assert!(tokenize("1 $ 2").is_err());
}
EOF
commit chidi 20 "Add tests for the lexer"
sed -i 's|            Some(t) => Err(format!("unexpected {t:?}")),|            Some(t) => Err(format!("unexpected {t:?} after the expression")),|' src/parser.rs
commit chidi 16 "Parser: say where the expression ended"
git checkout -q -b fix/unary-minus
sed -i 's|            Some(Token::Minus) => Ok(Expr::Neg(Box::new(self.factor()?))),|            Some(Token::Minus) => Ok(Expr::Neg(Box::new(self.factor()?))), // binds tighter than * and /|' src/parser.rs
commit chidi 4 "Parser: document how unary minus binds"
git checkout -q main
merge 12 feature/eval
sed -i 's|            other => Err(format!("unexpected {other:?}")),|            Some(t) => Err(format!("unexpected {t:?} where a value was expected")),\n            None => Err("unexpected end of input".into()),|' src/parser.rs
commit ann 6 "Parser: better error messages"
git tag -a v0.1.0 -m "calc 0.1.0"

git init -q --bare "$dir.origin.git"
git remote add origin "$dir.origin.git"
git push -q -u origin main

# one commit that is not pushed yet, then the working tree
sed -i 's|^    pos: usize,|    pos: usize,\n    line: usize,|; s|Parser { tokens, pos: 0 }|Parser { tokens, pos: 0, line: 1 }|' src/parser.rs
commit ben 2 "Parser: remember the line"

# unstaged: a changed line, an added function, removed lines (src/parser.rs) and a README edit
sed -i 's|    fn peek(&self) -> Option<&Token> {|    /// The next token, without consuming it.\n    fn peek(\&self) -> Option<\&Token> {|' src/parser.rs
sed -i '0,/        let e = self.expr()?;/s||        let e = self.expr().map_err(\|msg\| format!("line {}: {msg}", self.line))?;|' src/parser.rs
sed -i '/^    fn next(&mut self) -> Option<Token> {$/,/^    }$/c\    fn next(\&mut self) -> Option<Token> {\n        self.pos += 1;\n        self.tokens.get(self.pos - 1).cloned()\n    }' src/parser.rs
cat >>src/parser.rs <<'EOF'

impl Expr {
    /// How deeply the tree nests.
    pub fn depth(&self) -> usize {
        match self {
            Expr::Number(_) => 1,
            Expr::Neg(e) => 1 + e.depth(),
            Expr::Binary(l, _, r) => 1 + l.depth().max(r.depth()),
        }
    }
}
EOF
sed -i 's|^A tiny expression calculator: .*|A tiny expression calculator: a lexer, a precedence-climbing parser and an evaluator.\n\nRun `cargo test` to check it.|' README.md
# staged: one lexer change
sed -i 's|            other => return Err(format!("unexpected {other}")),|            other => return Err(format!("unexpected character {other:?}")),|' src/lexer.rs
git add src/lexer.rs
# untracked
cat >src/span.rs <<'EOF'
//! Where a token starts and ends.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}
EOF
