// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! A small expression language for extracting numeric values (histogram
//! axes) or boolean filters (nonzero = keep) from an [`Event`].
//!
//! Grammar:
//! ```text
//! expr    := or
//! or      := and ( "||" and )*
//! and     := cmp ( "&&" cmp )*
//! cmp     := add ( ("=="|"!="|"<"|"<="|">"|">=") add )?
//! add     := mul ( ("+"|"-") mul )*
//! mul     := unary ( ("*"|"/"|"&"|">>"|"<<") unary )*
//! unary   := ("-"|"!") unary | postfix
//! postfix := primary ( "[" int ".." int (":signed")? "]" )*
//! primary := int | ident | "(" expr ")"
//! ```
//!
//! Fields: `time`, `rel_time`, `raw_0`, `raw_1`, `channel`, `ampl`,
//! `x`, `y`, `t`, `i`, `flags`, `evtype`, `auxnum`, `monnum`, `gateup`.
//!
//! Named constants for `evtype` comparisons:
//! `neutron`, `monitor`, `edge`, `gate`, `tzero`, `auxsignal`, `heartbeat`,
//! `void` (matching [`EventType`]'s `#[repr(u8)]` discriminants).
//!
//! Integer literals: decimal (`100`), hex (`0xFF`), or binary (`0b1010`).
//!
//! Bit-slice: `<expr>[offset..end]` (unsigned) or `[offset..end:signed]`
//! (sign-extended), e.g. `raw_0[0..12:signed]` extracts the low 12 bits of
//! `raw_0` as a signed integer.
//!
//! An identifier that isn't a known field or named constant may be resolved
//! against a caller-supplied alias table (name -> expression text), e.g. an
//! input recipe contributing `adc0` for `raw_0[0..12:signed]`; see
//! [`Expr::parse_with_aliases`].

use std::collections::BTreeMap;
use anyhow::{anyhow, bail, Context};
use crate::event::{Event, EventFlags, EventType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Time,
    RelTime,
    Raw0,
    Raw1,
    Channel,
    Ampl,
    HistoX,
    HistoY,
    HistoT,
    HistoI,
    Flags,
    EvType,
    AuxNum,
    MonNum,
    GateUp,
}

impl Field {
    fn resolve(ident: &str) -> Option<Self> {
        Some(match ident {
            "time" => Field::Time,
            "rel_time" => Field::RelTime,
            "raw_0" => Field::Raw0,
            "raw_1" => Field::Raw1,
            "channel" => Field::Channel,
            "ampl" => Field::Ampl,
            "x" => Field::HistoX,
            "y" => Field::HistoY,
            "t" => Field::HistoT,
            "i" => Field::HistoI,
            "flags" => Field::Flags,
            "evtype" => Field::EvType,
            "auxnum" => Field::AuxNum,
            "monnum" => Field::MonNum,
            "gateup" => Field::GateUp,
            _ => return None,
        })
    }

    fn eval(self, ev: &Event) -> i64 {
        match self {
            Field::Time => ev.time.0,
            Field::RelTime => ev.rel_time.0,
            Field::Raw0 => ev.raw.0 as i64,
            Field::Raw1 => ev.raw.1 as i64,
            Field::Channel => ev.channel.0 as i64,
            Field::Ampl => ev.ampl.0 as i64,
            Field::HistoX => ev.histo.x as i64,
            Field::HistoY => ev.histo.y as i64,
            Field::HistoT => ev.histo.t as i64,
            Field::HistoI => ev.histo.i as i64,
            Field::Flags => {
                (ev.flags.contains(EventFlags::HasRelTime) as i64)
                    | ((ev.flags.contains(EventFlags::Fake) as i64) << 12)
            }
            Field::EvType => evtype_discriminant(&ev.evtype),
            Field::AuxNum => match ev.evtype {
                EventType::AuxSignal { num } => num as i64,
                _ => -1,
            },
            Field::MonNum => match ev.evtype {
                EventType::Monitor { num } => num as i64,
                _ => -1,
            },
            Field::GateUp => match ev.evtype {
                EventType::Edge { up } | EventType::Gate { up } => up as i64,
                _ => -1,
            },
        }
    }
}

fn evtype_discriminant(t: &EventType) -> i64 {
    match t {
        EventType::Neutron => 0x01,
        EventType::Monitor { .. } => 0x02,
        EventType::Edge { .. } => 0x10,
        EventType::Gate { .. } => 0x11,
        EventType::Tzero => 0x12,
        EventType::AuxSignal { .. } => 0x13,
        EventType::Heartbeat => 0x80,
        EventType::Void => 0xFF,
    }
}

fn named_constant(ident: &str) -> Option<i64> {
    Some(match ident {
        "neutron" => 0x01,
        "monitor" => 0x02,
        "edge" => 0x10,
        "gate" => 0x11,
        "tzero" => 0x12,
        "auxsignal" => 0x13,
        "heartbeat" => 0x80,
        "void" => 0xFF,
        _ => return None,
    })
}

/// A named expression alias contributed by a recipe or the config file
/// (e.g. `adc0` -> `raw_0[0..12:signed]`), with a short description for
/// user-facing help text.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExprAlias {
    pub name: String,
    pub expr: String,
    pub help: String,
}

impl ExprAlias {
    pub fn new(name: &str, expr: &str, help: &str) -> Self {
        ExprAlias { name: name.into(), expr: expr.into(), help: help.into() }
    }
}

pub type AliasTable = BTreeMap<String, ExprAlias>;

/// True if `name` is an intrinsic field or named constant, which a custom
/// alias must not shadow.
pub fn is_reserved_name(name: &str) -> bool {
    Field::resolve(name).is_some() || named_constant(name).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinOp {
    Add, Sub, Mul, Div, BitAnd, Shl, Shr,
    Eq, Ne, Lt, Le, Gt, Ge,
    LogAnd, LogOr,
}

#[derive(Debug, Clone)]
enum Node {
    Lit(i64),
    Field(Field),
    Slice(Box<Node>, u32, u32, bool),
    Neg(Box<Node>),
    Not(Box<Node>),
    BinOp(BinOp, Box<Node>, Box<Node>),
}

impl Node {
    fn eval(&self, ev: &Event) -> i64 {
        match self {
            Node::Lit(v) => *v,
            Node::Field(f) => f.eval(ev),
            Node::Slice(inner, offset, width, signed) => {
                let v = inner.eval(ev) as u32;
                let mask: u32 = if *width >= 32 { u32::MAX } else { (1u32 << width) - 1 };
                let extracted = (v >> offset) & mask;
                if *signed {
                    ((extracted as i32) << (32 - width) >> (32 - width)) as i64
                } else {
                    extracted as i64
                }
            }
            Node::Neg(inner) => -inner.eval(ev),
            Node::Not(inner) => (inner.eval(ev) == 0) as i64,
            Node::BinOp(op, lhs, rhs) => {
                let l = lhs.eval(ev);
                match op {
                    BinOp::LogAnd => return ((l != 0) && (rhs.eval(ev) != 0)) as i64,
                    BinOp::LogOr => return ((l != 0) || (rhs.eval(ev) != 0)) as i64,
                    _ => {}
                }
                let r = rhs.eval(ev);
                match op {
                    BinOp::Add => l.wrapping_add(r),
                    BinOp::Sub => l.wrapping_sub(r),
                    BinOp::Mul => l.wrapping_mul(r),
                    BinOp::Div => if r == 0 { 0 } else { l.wrapping_div(r) },
                    BinOp::BitAnd => l & r,
                    BinOp::Shl => l.wrapping_shl(r as u32),
                    BinOp::Shr => l.wrapping_shr(r as u32),
                    BinOp::Eq => (l == r) as i64,
                    BinOp::Ne => (l != r) as i64,
                    BinOp::Lt => (l < r) as i64,
                    BinOp::Le => (l <= r) as i64,
                    BinOp::Gt => (l > r) as i64,
                    BinOp::Ge => (l >= r) as i64,
                    BinOp::LogAnd | BinOp::LogOr => unreachable!("handled above"),
                }
            }
        }
    }
}

/// A parsed, ready-to-evaluate expression.
#[derive(Debug, Clone)]
pub struct Expr {
    root: Node,
}

/// Nested alias expansion is capped at this depth to catch an accidental
/// cycle (e.g. two aliases defined in terms of each other).
const MAX_ALIAS_DEPTH: u32 = 8;

impl Expr {
    pub fn parse(src: &str) -> anyhow::Result<Self> {
        Self::parse_with_aliases(src, &AliasTable::new())
    }

    /// Like [`Expr::parse`], but resolves any identifier that isn't a known
    /// field or named constant against `aliases` before giving up. Aliases
    /// may reference other aliases.
    pub fn parse_with_aliases(src: &str, aliases: &AliasTable) -> anyhow::Result<Self> {
        Self::parse_inner(src, aliases, 0)
    }

    fn parse_inner(src: &str, aliases: &AliasTable, depth: u32) -> anyhow::Result<Self> {
        if depth > MAX_ALIAS_DEPTH {
            bail!("Alias expansion nested too deeply (possible cycle) while parsing {src:?}");
        }
        let toks = tokenize(src).with_context(|| format!("Tokenizing expression {src:?}"))?;
        let mut p = Parser { toks: &toks, pos: 0, aliases, depth };
        let root = p.parse_or().with_context(|| format!("Parsing expression {src:?}"))?;
        if p.pos != p.toks.len() {
            bail!("Unexpected trailing input in expression {src:?} at token {}", p.pos);
        }
        Ok(Expr { root })
    }

    pub fn eval(&self, ev: &Event) -> i64 {
        self.root.eval(ev)
    }
}

// ---- tokenizer ----

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Int(i64),
    Op(&'static str),
}

fn tokenize(src: &str) -> anyhow::Result<Vec<Tok>> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut toks = Vec::new();
    // longest-match-first two-char operators
    const TWO_CHAR: &[(&str, &str)] = &[
        ("==", "=="), ("!=", "!="), ("<=", "<="), (">=", ">="),
        ("&&", "&&"), ("||", "||"), ("<<", "<<"), (">>", ">>"), ("..", ".."),
    ];
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            if c == '0' && chars.get(i + 1).is_some_and(|&n| n == 'x' || n == 'X') {
                i += 2;
                let hstart = i;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let hex: String = chars[hstart..i].iter().collect();
                let v = i64::from_str_radix(&hex, 16)
                    .with_context(|| format!("Invalid hex literal at position {start}"))?;
                toks.push(Tok::Int(v));
                continue;
            }
            if c == '0' && chars.get(i + 1).is_some_and(|&n| n == 'b' || n == 'B') {
                i += 2;
                let bstart = i;
                while i < chars.len() && (chars[i] == '0' || chars[i] == '1') {
                    i += 1;
                }
                let bin: String = chars[bstart..i].iter().collect();
                let v = i64::from_str_radix(&bin, 2)
                    .with_context(|| format!("Invalid binary literal at position {start}"))?;
                toks.push(Tok::Int(v));
                continue;
            }
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            let v: i64 = s.parse().with_context(|| format!("Invalid integer literal {s:?}"))?;
            toks.push(Tok::Int(v));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            toks.push(Tok::Ident(chars[start..i].iter().collect()));
            continue;
        }
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        if let Some((_, op)) = TWO_CHAR.iter().find(|(pat, _)| *pat == two) {
            toks.push(Tok::Op(op));
            i += 2;
            continue;
        }
        let op = match c {
            '+' => "+", '-' => "-", '*' => "*", '/' => "/", '&' => "&",
            '<' => "<", '>' => ">", '!' => "!", '(' => "(", ')' => ")",
            '[' => "[", ']' => "]", ':' => ":", '=' => bail!("Unexpected '=' (did you mean '=='?) at position {i}"),
            other => bail!("Unexpected character {other:?} at position {i}"),
        };
        toks.push(Tok::Op(op));
        i += 1;
    }
    Ok(toks)
}

// ---- recursive-descent parser ----

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
    aliases: &'a AliasTable,
    depth: u32,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_op(&self, op: &str) -> bool {
        matches!(self.peek(), Some(Tok::Op(o)) if *o == op)
    }

    fn bump(&mut self) -> Option<&Tok> {
        let t = self.toks.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect_op(&mut self, op: &str) -> anyhow::Result<()> {
        if self.peek_op(op) {
            self.pos += 1;
            Ok(())
        } else {
            bail!("Expected {op:?} at token {}, found {:?}", self.pos, self.peek())
        }
    }

    fn parse_or(&mut self) -> anyhow::Result<Node> {
        let mut node = self.parse_and()?;
        while self.peek_op("||") {
            self.pos += 1;
            let rhs = self.parse_and()?;
            node = Node::BinOp(BinOp::LogOr, Box::new(node), Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_and(&mut self) -> anyhow::Result<Node> {
        let mut node = self.parse_cmp()?;
        while self.peek_op("&&") {
            self.pos += 1;
            let rhs = self.parse_cmp()?;
            node = Node::BinOp(BinOp::LogAnd, Box::new(node), Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_cmp(&mut self) -> anyhow::Result<Node> {
        let node = self.parse_add()?;
        let op = match self.peek() {
            Some(Tok::Op("==")) => BinOp::Eq,
            Some(Tok::Op("!=")) => BinOp::Ne,
            Some(Tok::Op("<")) => BinOp::Lt,
            Some(Tok::Op("<=")) => BinOp::Le,
            Some(Tok::Op(">")) => BinOp::Gt,
            Some(Tok::Op(">=")) => BinOp::Ge,
            _ => return Ok(node),
        };
        self.pos += 1;
        let rhs = self.parse_add()?;
        Ok(Node::BinOp(op, Box::new(node), Box::new(rhs)))
    }

    fn parse_add(&mut self) -> anyhow::Result<Node> {
        let mut node = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op("+")) => BinOp::Add,
                Some(Tok::Op("-")) => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_mul()?;
            node = Node::BinOp(op, Box::new(node), Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_mul(&mut self) -> anyhow::Result<Node> {
        let mut node = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op("*")) => BinOp::Mul,
                Some(Tok::Op("/")) => BinOp::Div,
                Some(Tok::Op("&")) => BinOp::BitAnd,
                Some(Tok::Op(">>")) => BinOp::Shr,
                Some(Tok::Op("<<")) => BinOp::Shl,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_unary()?;
            node = Node::BinOp(op, Box::new(node), Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_unary(&mut self) -> anyhow::Result<Node> {
        if self.peek_op("-") {
            self.pos += 1;
            return Ok(Node::Neg(Box::new(self.parse_unary()?)));
        }
        if self.peek_op("!") {
            self.pos += 1;
            return Ok(Node::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> anyhow::Result<Node> {
        let mut node = self.parse_primary()?;
        while self.peek_op("[") {
            self.pos += 1;
            let offset = self.parse_int_literal()?;
            self.expect_op("..")?;
            let end = self.parse_int_literal()?;
            if end <= offset || end - offset > 32 || offset >= 32 {
                bail!("Invalid bit slice [{offset}..{end}): must be non-empty, within 32 bits");
            }
            let signed = if self.peek_op(":") {
                self.pos += 1;
                match self.bump() {
                    Some(Tok::Ident(s)) if s == "signed" => true,
                    other => bail!("Expected 'signed' after ':' in bit slice, found {other:?}"),
                }
            } else {
                false
            };
            self.expect_op("]")?;
            node = Node::Slice(Box::new(node), offset as u32, (end - offset) as u32, signed);
        }
        Ok(node)
    }

    fn parse_int_literal(&mut self) -> anyhow::Result<i64> {
        match self.bump() {
            Some(Tok::Int(v)) => Ok(*v),
            other => bail!("Expected integer literal, found {other:?}"),
        }
    }

    fn parse_primary(&mut self) -> anyhow::Result<Node> {
        match self.bump() {
            Some(Tok::Int(v)) => Ok(Node::Lit(*v)),
            Some(Tok::Ident(id)) => {
                let id = id.clone();
                if let Some(f) = Field::resolve(&id) {
                    Ok(Node::Field(f))
                } else if let Some(c) = named_constant(&id) {
                    Ok(Node::Lit(c))
                } else if let Some(alias) = self.aliases.get(&id) {
                    Expr::parse_inner(&alias.expr, self.aliases, self.depth + 1)
                        .map(|e| e.root)
                        .with_context(|| format!("Expanding alias {id:?}"))
                } else {
                    Err(anyhow!("Unknown identifier {id:?}"))
                }
            }
            Some(Tok::Op("(")) => {
                let node = self.parse_or()?;
                self.expect_op(")")?;
                Ok(node)
            }
            other => bail!("Expected a value, found {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::test_utils;

    fn ev_with(f: impl FnOnce(&mut Event)) -> Event {
        let mut ev = test_utils::neutron(0, 0);
        f(&mut ev);
        ev
    }

    #[test]
    fn test_field_access() {
        let ev = ev_with(|e| {
            e.time = crate::event::EventTime(100);
            e.rel_time = crate::event::EventTime(50);
            e.raw = (0xDEAD_BEEF, 0x1234_5678);
            e.channel = crate::event::ChannelId(42);
            e.ampl = crate::event::Amplitude(7);
            e.histo.x = 1;
            e.histo.y = 2;
            e.histo.t = 3;
            e.histo.i = 4;
        });
        assert_eq!(Expr::parse("time").unwrap().eval(&ev), 100);
        assert_eq!(Expr::parse("rel_time").unwrap().eval(&ev), 50);
        assert_eq!(Expr::parse("raw_0").unwrap().eval(&ev), 0xDEAD_BEEFu32 as i64);
        assert_eq!(Expr::parse("raw_1").unwrap().eval(&ev), 0x1234_5678);
        assert_eq!(Expr::parse("channel").unwrap().eval(&ev), 42);
        assert_eq!(Expr::parse("ampl").unwrap().eval(&ev), 7);
        assert_eq!(Expr::parse("x").unwrap().eval(&ev), 1);
        assert_eq!(Expr::parse("y").unwrap().eval(&ev), 2);
        assert_eq!(Expr::parse("t").unwrap().eval(&ev), 3);
        assert_eq!(Expr::parse("i").unwrap().eval(&ev), 4);
    }

    #[test]
    fn test_evtype_and_named_constants() {
        let neutron = test_utils::neutron(0, 0);
        assert_eq!(Expr::parse("evtype").unwrap().eval(&neutron), 0x01);
        assert_eq!(Expr::parse("evtype == neutron").unwrap().eval(&neutron), 1);
        assert_eq!(Expr::parse("evtype == monitor").unwrap().eval(&neutron), 0);

        let tzero = test_utils::tzero(0);
        assert_eq!(Expr::parse("evtype == tzero").unwrap().eval(&tzero), 1);

        let aux = test_utils::aux(0, 3);
        assert_eq!(Expr::parse("evtype == auxsignal").unwrap().eval(&aux), 1);
        assert_eq!(Expr::parse("auxnum").unwrap().eval(&aux), 3);
        assert_eq!(Expr::parse("auxnum").unwrap().eval(&neutron), -1);

        let monitor = test_utils::monitor(0, 2);
        assert_eq!(Expr::parse("evtype == monitor").unwrap().eval(&monitor), 1);
        assert_eq!(Expr::parse("monnum").unwrap().eval(&monitor), 2);
        assert_eq!(Expr::parse("monnum").unwrap().eval(&neutron), -1);

        let edge_up = test_utils::edge(0, 0, true);
        assert_eq!(Expr::parse("evtype == edge").unwrap().eval(&edge_up), 1);
        assert_eq!(Expr::parse("gateup").unwrap().eval(&edge_up), 1);
        let gate_down = test_utils::gate(0, false);
        assert_eq!(Expr::parse("gateup").unwrap().eval(&gate_down), 0);
        assert_eq!(Expr::parse("gateup").unwrap().eval(&neutron), -1);
    }

    #[test]
    fn test_flags() {
        let mut ev = test_utils::neutron(0, 0);
        assert_eq!(Expr::parse("flags").unwrap().eval(&ev), 0);
        ev.flags.set(EventFlags::HasRelTime);
        assert_eq!(Expr::parse("flags").unwrap().eval(&ev), 1);
        ev.flags.set(EventFlags::Fake);
        assert_eq!(Expr::parse("flags").unwrap().eval(&ev), 1 | 0x1000);
    }

    #[test]
    fn test_bit_slice_unsigned_and_signed() {
        // Jumiom-style: word2 packs adc0 (bits 0..12) and adc1 (bits 16..28).
        let enc = |v: i32| -> u32 { (v as u32) & 0xFFF };
        let word = (enc(-1) << 16) | enc(100);
        let ev = ev_with(|e| e.raw = (word, 0));
        assert_eq!(Expr::parse("raw_0[0..12]").unwrap().eval(&ev), 100);
        assert_eq!(Expr::parse("raw_0[0..12:signed]").unwrap().eval(&ev), 100);
        assert_eq!(Expr::parse("raw_0[16..28:signed]").unwrap().eval(&ev), -1);
        // unsigned view of the all-ones 12-bit field is 0xFFF = 4095
        assert_eq!(Expr::parse("raw_0[16..28]").unwrap().eval(&ev), 0xFFF);
    }

    #[test]
    fn test_arithmetic_and_precedence() {
        let ev = test_utils::neutron(0, 0);
        assert_eq!(Expr::parse("1 + 2 * 3").unwrap().eval(&ev), 7);
        assert_eq!(Expr::parse("(1 + 2) * 3").unwrap().eval(&ev), 9);
        assert_eq!(Expr::parse("10 / 4").unwrap().eval(&ev), 2);
        assert_eq!(Expr::parse("-5 + 3").unwrap().eval(&ev), -2);
        assert_eq!(Expr::parse("1 << 4").unwrap().eval(&ev), 16);
        assert_eq!(Expr::parse("0xFF & 0x0F").unwrap().eval(&ev), 0x0F);
    }

    #[test]
    fn test_number_literal_formats() {
        let ev = test_utils::neutron(0, 0);
        assert_eq!(Expr::parse("42").unwrap().eval(&ev), 42);
        assert_eq!(Expr::parse("0x2A").unwrap().eval(&ev), 42);
        assert_eq!(Expr::parse("0b101010").unwrap().eval(&ev), 42);
    }

    #[test]
    fn test_logical_operators() {
        let neutron = test_utils::neutron(0, 0);
        assert_eq!(Expr::parse("evtype == neutron && 1 < 2").unwrap().eval(&neutron), 1);
        assert_eq!(Expr::parse("evtype == neutron && 1 > 2").unwrap().eval(&neutron), 0);
        assert_eq!(Expr::parse("evtype == monitor || evtype == neutron").unwrap().eval(&neutron), 1);
        assert_eq!(Expr::parse("!(evtype == neutron)").unwrap().eval(&neutron), 0);
        assert_eq!(Expr::parse("!(evtype == monitor)").unwrap().eval(&neutron), 1);
    }

    #[test]
    fn test_parse_errors() {
        assert!(Expr::parse("bogus_field").is_err());
        assert!(Expr::parse("raw_0[0..40]").is_err()); // exceeds 32 bits
        assert!(Expr::parse("raw_0[5..2]").is_err()); // empty/negative range
        assert!(Expr::parse("(1 + 2").is_err()); // unbalanced parens
        assert!(Expr::parse("1 +").is_err()); // trailing operator
        assert!(Expr::parse("1 2").is_err()); // trailing tokens
    }

    fn alias(name: &str, expr: &str) -> ExprAlias {
        ExprAlias::new(name, expr, "")
    }

    #[test]
    fn test_alias_expansion() {
        let enc = |v: i32| -> u32 { (v as u32) & 0xFFF };
        let word = (enc(-1) << 16) | enc(100);
        let ev = ev_with(|e| e.raw = (word, 0));
        let mut aliases = AliasTable::new();
        aliases.insert("adc0".into(), alias("adc0", "raw_0[0..12:signed]"));
        aliases.insert("adc1".into(), alias("adc1", "raw_0[16..28:signed]"));
        assert_eq!(Expr::parse_with_aliases("adc0", &aliases).unwrap().eval(&ev), 100);
        assert_eq!(Expr::parse_with_aliases("adc1", &aliases).unwrap().eval(&ev), -1);
        // aliases compose with the rest of the grammar
        assert_eq!(Expr::parse_with_aliases("adc0 + 1", &aliases).unwrap().eval(&ev), 101);
    }

    #[test]
    fn test_alias_referencing_alias() {
        let mut ev = test_utils::neutron(0, 0);
        ev.channel = crate::event::ChannelId(20);
        let mut aliases = AliasTable::new();
        aliases.insert("half".into(), alias("half", "channel / 2"));
        aliases.insert("quarter".into(), alias("quarter", "half / 2"));
        assert_eq!(Expr::parse_with_aliases("quarter", &aliases).unwrap().eval(&ev), 5);
    }

    #[test]
    fn test_alias_cycle_is_rejected() {
        let mut aliases = AliasTable::new();
        aliases.insert("a".into(), alias("a", "b"));
        aliases.insert("b".into(), alias("b", "a"));
        assert!(Expr::parse_with_aliases("a", &aliases).is_err());
    }

    #[test]
    fn test_unknown_identifier_with_empty_alias_table_is_still_an_error() {
        assert!(Expr::parse_with_aliases("bogus", &AliasTable::new()).is_err());
    }

    #[test]
    fn test_is_reserved_name() {
        for name in ["time", "raw_0", "x", "evtype", "neutron", "gateup"] {
            assert!(is_reserved_name(name), "{name:?} should be reserved");
        }
        for name in ["adc0", "half", "my_alias"] {
            assert!(!is_reserved_name(name), "{name:?} should not be reserved");
        }
    }
}
