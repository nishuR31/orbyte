//! Hand-written lexer for the JavaScript/TypeScript subset.
//!
//! Supports string literals (single and double quoted with escape sequences),
//! hex/binary/octal numeric literals, numeric separators, and Unicode
//! identifiers. Each token records whether a line break preceded it, which the
//! parser uses for automatic semicolon insertion.

use diagnostics::{Diagnostic, Result, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Number(f64),
    /// A string literal with its decoded value (escape sequences resolved).
    Str(String),
    Ident(String),
    // keywords
    Function,
    Let,
    Const,
    Var,
    If,
    Else,
    While,
    For,
    Break,
    Continue,
    Return,
    True,
    False,
    Null,
    Undefined,
    Typeof,
    // TypeScript keywords
    Type,
    Interface,
    Export,
    Import,
    From,
    As,
    // punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Dot,
    Colon,
    Question,
    NullishCoalescing,
    OptionalChain,
    Pipe,
    Amp,
    // operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Assign,
    FatArrow,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    PlusPlus,
    MinusMinus,
    AmpAmp,
    PipePipe,
    EqEq,
    EqEqEq,
    BangEq,
    BangEqEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Eof,
}

impl TokenKind {
    fn text(&self) -> &'static str {
        use TokenKind as T;
        match self {
            T::Function => "function",
            T::Let => "let",
            T::Const => "const",
            T::Var => "var",
            T::If => "if",
            T::Else => "else",
            T::While => "while",
            T::For => "for",
            T::Break => "break",
            T::Continue => "continue",
            T::Return => "return",
            T::True => "true",
            T::False => "false",
            T::Null => "null",
            T::Undefined => "undefined",
            T::Typeof => "typeof",
            T::Type => "type",
            T::Interface => "interface",
            T::Export => "export",
            T::Import => "import",
            T::From => "from",
            T::As => "as",
            T::LParen => "(",
            T::RParen => ")",
            T::LBrace => "{",
            T::RBrace => "}",
            T::LBracket => "[",
            T::RBracket => "]",
            T::Comma => ",",
            T::Semi => ";",
            T::Dot => ".",
            T::Colon => ":",
            T::Question => "?",
            T::NullishCoalescing => "??",
            T::OptionalChain => "?.",
            T::Pipe => "|",
            T::Amp => "&",
            T::Plus => "+",
            T::Minus => "-",
            T::Star => "*",
            T::Slash => "/",
            T::Percent => "%",
            T::Bang => "!",
            T::Assign => "=",
            T::FatArrow => "=>",
            T::PlusEq => "+=",
            T::MinusEq => "-=",
            T::StarEq => "*=",
            T::SlashEq => "/=",
            T::PercentEq => "%=",
            T::PlusPlus => "++",
            T::MinusMinus => "--",
            T::AmpAmp => "&&",
            T::PipePipe => "||",
            T::EqEq => "==",
            T::EqEqEq => "===",
            T::BangEq => "!=",
            T::BangEqEq => "!==",
            T::Lt => "<",
            T::LtEq => "<=",
            T::Gt => ">",
            T::GtEq => ">=",
            T::Number(_) | T::Ident(_) | T::Str(_) | T::Eof => "",
        }
    }

    /// Human-readable description for diagnostics.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Number(n) => format!("number '{n}'"),
            TokenKind::Str(s) => format!("string '{s}'"),
            TokenKind::Ident(n) => format!("identifier '{n}'"),
            TokenKind::Eof => "end of file".to_string(),
            other => format!("'{}'", other.text()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// True if at least one line break separates this token from the previous one.
    pub newline_before: bool,
}

/// True if `c` can start an ASCII identifier (fast path for ASCII characters).
fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$'
}

/// True if `ch` can start a Unicode identifier (ECMAScript IdentifierStart).
fn is_ident_start_unicode(ch: char) -> bool {
    ch == '_' || ch == '$' || unicode_ident::is_xid_start(ch)
}

/// True if `c` can continue an ASCII identifier (fast path).
fn is_ident_part(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

/// True if `ch` can continue a Unicode identifier (ECMAScript IdentifierPart).
fn is_ident_part_unicode(ch: char) -> bool {
    ch == '$' || unicode_ident::is_xid_continue(ch)
}

fn keyword(text: &str) -> Option<TokenKind> {
    Some(match text {
        "function" => TokenKind::Function,
        "let" => TokenKind::Let,
        "const" => TokenKind::Const,
        "var" => TokenKind::Var,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "for" => TokenKind::For,
        "break" => TokenKind::Break,
        "continue" => TokenKind::Continue,
        "return" => TokenKind::Return,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "null" => TokenKind::Null,
        "undefined" => TokenKind::Undefined,
        "typeof" => TokenKind::Typeof,
        "type" => TokenKind::Type,
        "interface" => TokenKind::Interface,
        "export" => TokenKind::Export,
        "import" => TokenKind::Import,
        "from" => TokenKind::From,
        "as" => TokenKind::As,
        _ => return None,
    })
}

/// True if the UTF-8 bytes at `i` encode U+2028 (LINE SEPARATOR) or U+2029
/// (PARAGRAPH SEPARATOR), both of which are ECMAScript line terminators.
fn line_separator_at(b: &[u8], i: usize) -> bool {
    b.get(i) == Some(&0xE2) && b.get(i + 1) == Some(&0x80) && matches!(b.get(i + 2), Some(0xA8) | Some(0xA9))
}

pub fn tokenize(src: &str) -> Result<Vec<Token>> {
    use TokenKind as T;

    let b = src.as_bytes();
    let mut i = 0usize;
    let mut nl = false;
    let mut out: Vec<Token> = Vec::new();

    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' => i += 1,
            // Line terminators: LF, CR (alone or as part of CRLF), U+2028 and U+2029.
            b'\n' | b'\r' => {
                nl = true;
                i += 1;
            }
            0xE2 if line_separator_at(b, i) => {
                nl = true;
                i += 3;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' && b[i] != b'\r' && !line_separator_at(b, i) {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let start = i;
                i += 2;
                loop {
                    if i + 1 >= b.len() {
                        return Err(Diagnostic::new(
                            "unterminated block comment",
                            Span::new(start, start + 2),
                        ));
                    }
                    if b[i] == b'*' && b[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    if b[i] == b'\n' || b[i] == b'\r' || line_separator_at(b, i) {
                        nl = true;
                    }
                    i += 1;
                }
            }
            c if c.is_ascii_digit()
                || (c == b'.' && b.get(i + 1).map_or(false, |d| d.is_ascii_digit())) =>
            {
                let start = i;

                // ---- Hex / binary / octal prefixes ----
                if c == b'0' {
                    match b.get(i + 1) {
                        Some(&b'x') | Some(&b'X') => {
                            i += 2;
                            let hex_start = i;
                            while i < b.len()
                                && (b[i].is_ascii_hexdigit() || b[i] == b'_')
                            {
                                i += 1;
                            }
                            if i == hex_start {
                                return Err(Diagnostic::new(
                                    "expected hex digits after '0x'",
                                    Span::new(start, i),
                                ));
                            }
                            let raw = &src[hex_start..i];
                            let clean: String =
                                raw.chars().filter(|&c| c != '_').collect();
                            if clean.is_empty() {
                                return Err(Diagnostic::new(
                                    "numeric separator cannot appear here",
                                    Span::new(start, i),
                                ));
                            }
                            let value = u64::from_str_radix(&clean, 16).map_err(|_| {
                                Diagnostic::new("invalid hex literal", Span::new(start, i))
                            })? as f64;
                            out.push(Token {
                                kind: T::Number(value),
                                span: Span::new(start, i),
                                newline_before: nl,
                            });
                            nl = false;
                            continue;
                        }
                        Some(&b'b') | Some(&b'B') => {
                            i += 2;
                            let bin_start = i;
                            while i < b.len()
                                && (b[i] == b'0' || b[i] == b'1' || b[i] == b'_')
                            {
                                i += 1;
                            }
                            if i == bin_start {
                                return Err(Diagnostic::new(
                                    "expected binary digits after '0b'",
                                    Span::new(start, i),
                                ));
                            }
                            let raw = &src[bin_start..i];
                            let clean: String =
                                raw.chars().filter(|&c| c != '_').collect();
                            if clean.is_empty() {
                                return Err(Diagnostic::new(
                                    "numeric separator cannot appear here",
                                    Span::new(start, i),
                                ));
                            }
                            let value = u64::from_str_radix(&clean, 2).map_err(|_| {
                                Diagnostic::new("invalid binary literal", Span::new(start, i))
                            })? as f64;
                            out.push(Token {
                                kind: T::Number(value),
                                span: Span::new(start, i),
                                newline_before: nl,
                            });
                            nl = false;
                            continue;
                        }
                        Some(&b'o') | Some(&b'O') => {
                            i += 2;
                            let oct_start = i;
                            while i < b.len()
                                && ((b[i] >= b'0' && b[i] <= b'7') || b[i] == b'_')
                            {
                                i += 1;
                            }
                            if i == oct_start {
                                return Err(Diagnostic::new(
                                    "expected octal digits after '0o'",
                                    Span::new(start, i),
                                ));
                            }
                            let raw = &src[oct_start..i];
                            let clean: String =
                                raw.chars().filter(|&c| c != '_').collect();
                            if clean.is_empty() {
                                return Err(Diagnostic::new(
                                    "numeric separator cannot appear here",
                                    Span::new(start, i),
                                ));
                            }
                            let value = u64::from_str_radix(&clean, 8).map_err(|_| {
                                Diagnostic::new("invalid octal literal", Span::new(start, i))
                            })? as f64;
                            out.push(Token {
                                kind: T::Number(value),
                                span: Span::new(start, i),
                                newline_before: nl,
                            });
                            nl = false;
                            continue;
                        }
                        _ => {}
                    }
                }

                // ---- Decimal (with optional numeric separators) ----
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
                if i < b.len() && b[i] == b'.' {
                    i += 1;
                    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                        i += 1;
                    }
                }
                if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                    let mut j = i + 1;
                    if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                        j += 1;
                    }
                    if j < b.len() && b[j].is_ascii_digit() {
                        while j < b.len() && b[j].is_ascii_digit() {
                            j += 1;
                        }
                        i = j;
                    } else {
                        return Err(Diagnostic::new(
                            "malformed exponent in number literal",
                            Span::new(start, j),
                        ));
                    }
                }
                if i < b.len() && is_ident_start(b[i]) {
                    return Err(Diagnostic::new(
                        "identifier or unsupported literal syntax directly after a number",
                        Span::new(start, i + 1),
                    ));
                }
                // Strip numeric separators before parsing.
                let raw = &src[start..i];
                let clean: String = raw.chars().filter(|&c| c != '_').collect();
                let value: f64 = clean
                    .parse()
                    .map_err(|_| Diagnostic::new("invalid number literal", Span::new(start, i)))?;
                out.push(Token { kind: T::Number(value), span: Span::new(start, i), newline_before: nl });
                nl = false;
            }
            // ---- String literals ----
            b'\'' | b'"' => {
                let quote = c;
                let start = i;
                i += 1; // consume opening quote
                let mut value = String::new();
                loop {
                    if i >= b.len() {
                        return Err(Diagnostic::new(
                            "unterminated string literal",
                            Span::new(start, i),
                        ));
                    }
                    let ch = b[i];
                    if ch == b'\n' || ch == b'\r' {
                        return Err(Diagnostic::new(
                            "unterminated string literal (line break inside string)",
                            Span::new(start, i),
                        ));
                    }
                    if ch == quote {
                        i += 1; // consume closing quote
                        break;
                    }
                    if ch == b'\\' {
                        i += 1;
                        if i >= b.len() {
                            return Err(Diagnostic::new(
                                "unterminated escape sequence",
                                Span::new(start, i),
                            ));
                        }
                        match b[i] {
                            b'n' => { value.push('\n'); i += 1; }
                            b't' => { value.push('\t'); i += 1; }
                            b'r' => { value.push('\r'); i += 1; }
                            b'0' => { value.push('\0'); i += 1; }
                            b'\\' => { value.push('\\'); i += 1; }
                            b'\'' => { value.push('\''); i += 1; }
                            b'"' => { value.push('"'); i += 1; }
                            b'b' => { value.push('\x08'); i += 1; }
                            b'f' => { value.push('\x0C'); i += 1; }
                            b'v' => { value.push('\x0B'); i += 1; }
                            b'x' => {
                                // \xNN — two hex digits
                                i += 1;
                                if i + 2 > b.len() {
                                    return Err(Diagnostic::new(
                                        "expected 2 hex digits after '\\x'",
                                        Span::new(start, i),
                                    ));
                                }
                                let hex = &src[i..i + 2];
                                let code = u32::from_str_radix(hex, 16).map_err(|_| {
                                    Diagnostic::new(
                                        format!("invalid hex escape '\\x{hex}'"),
                                        Span::new(start, i + 2),
                                    )
                                })?;
                                let ch = char::from_u32(code).ok_or_else(|| {
                                    Diagnostic::new("invalid Unicode scalar in \\x escape", Span::new(start, i + 2))
                                })?;
                                value.push(ch);
                                i += 2;
                            }
                            b'u' => {
                                i += 1;
                                if i < b.len() && b[i] == b'{' {
                                    // \u{XXXXX} — variable-length
                                    i += 1;
                                    let u_start = i;
                                    while i < b.len() && b[i].is_ascii_hexdigit() {
                                        i += 1;
                                    }
                                    if i >= b.len() || b[i] != b'}' {
                                        return Err(Diagnostic::new(
                                            "expected '}' after \\u{...",
                                            Span::new(start, i),
                                        ));
                                    }
                                    let hex = &src[u_start..i];
                                    let code = u32::from_str_radix(hex, 16).map_err(|_| {
                                        Diagnostic::new(
                                            format!("invalid Unicode escape '\\u{{{hex}}}'"),
                                            Span::new(start, i),
                                        )
                                    })?;
                                    let ch = char::from_u32(code).ok_or_else(|| {
                                        Diagnostic::new(
                                            "invalid Unicode scalar value",
                                            Span::new(start, i),
                                        )
                                    })?;
                                    value.push(ch);
                                    i += 1; // consume '}'
                                } else {
                                    // \uXXXX — exactly 4 hex digits
                                    if i + 4 > b.len() {
                                        return Err(Diagnostic::new(
                                            "expected 4 hex digits after '\\u'",
                                            Span::new(start, i),
                                        ));
                                    }
                                    let hex = &src[i..i + 4];
                                    let code = u32::from_str_radix(hex, 16).map_err(|_| {
                                        Diagnostic::new(
                                            format!("invalid Unicode escape '\\u{hex}'"),
                                            Span::new(start, i + 4),
                                        )
                                    })?;
                                    let ch = char::from_u32(code).ok_or_else(|| {
                                        Diagnostic::new(
                                            "invalid Unicode scalar value",
                                            Span::new(start, i + 4),
                                        )
                                    })?;
                                    value.push(ch);
                                    i += 4;
                                }
                            }
                            other => {
                                // Unknown escape: keep literal character (lenient).
                                let ch = src[i..].chars().next().unwrap_or('?');
                                let _ = other;
                                value.push(ch);
                                i += ch.len_utf8();
                            }
                        }
                    } else {
                        // Regular character (may be multi-byte UTF-8).
                        let ch = src[i..].chars().next().unwrap();
                        value.push(ch);
                        i += ch.len_utf8();
                    }
                }
                out.push(Token {
                    kind: T::Str(value),
                    span: Span::new(start, i),
                    newline_before: nl,
                });
                nl = false;
            }
            // ---- Template literals ----
            b'`' => {
                let start = i;
                i += 1; // consume opening backtick
                let mut value = String::new();
                let mut has_interpolation = false;

                enum Seg {
                    Str(String),
                    Expr(Vec<Token>),
                }
                let mut segments: Vec<Seg> = Vec::new();

                loop {
                    if i >= b.len() {
                        return Err(Diagnostic::new(
                            "unterminated template literal",
                            Span::new(start, i),
                        ));
                    }
                    let ch = b[i];
                    if ch == b'`' {
                        i += 1; // consume closing backtick
                        segments.push(Seg::Str(std::mem::take(&mut value)));
                        break;
                    }
                    if ch == b'$' && b.get(i + 1) == Some(&b'{') {
                        has_interpolation = true;
                        segments.push(Seg::Str(std::mem::take(&mut value)));
                        i += 2; // skip '${'
                        let expr_start = i;
                        let mut depth = 1usize;
                        let mut in_str: Option<u8> = None;
                        while i < b.len() {
                            if let Some(quote) = in_str {
                                if b[i] == b'\\' {
                                    i += 2;
                                    continue;
                                }
                                if b[i] == quote {
                                    in_str = None;
                                }
                            } else {
                                match b[i] {
                                    b'\'' | b'"' => in_str = Some(b[i]),
                                    b'{' => depth += 1,
                                    b'}' => {
                                        depth -= 1;
                                        if depth == 0 {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            i += 1;
                        }
                        if depth != 0 {
                            return Err(Diagnostic::new(
                                "unterminated '${' in template literal",
                                Span::new(expr_start - 2, i),
                            ));
                        }
                        let expr_src = &src[expr_start..i];
                        i += 1; // consume '}'
                        let inner_tokens = tokenize(expr_src)?;
                        let mut shifted = Vec::new();
                        for mut tok in inner_tokens {
                            if tok.kind != T::Eof {
                                tok.span.start += expr_start as u32;
                                tok.span.end += expr_start as u32;
                                shifted.push(tok);
                            }
                        }
                        segments.push(Seg::Expr(shifted));
                        continue;
                    }
                    if ch == b'\\' {
                        i += 1;
                        if i >= b.len() {
                            return Err(Diagnostic::new(
                                "unterminated escape sequence",
                                Span::new(start, i),
                            ));
                        }
                        match b[i] {
                            b'n' => { value.push('\n'); i += 1; }
                            b't' => { value.push('\t'); i += 1; }
                            b'r' => { value.push('\r'); i += 1; }
                            b'0' => { value.push('\0'); i += 1; }
                            b'\\' => { value.push('\\'); i += 1; }
                            b'`' => { value.push('`'); i += 1; }
                            b'$' => { value.push('$'); i += 1; }
                            _ => {
                                let c = src[i..].chars().next().unwrap_or('?');
                                value.push(c);
                                i += c.len_utf8();
                            }
                        }
                    } else {
                        let c = src[i..].chars().next().unwrap();
                        value.push(c);
                        i += c.len_utf8();
                    }
                }

                if !has_interpolation {
                    let text = match segments.into_iter().next() {
                        Some(Seg::Str(s)) => s,
                        _ => String::new(),
                    };
                    out.push(Token {
                        kind: T::Str(text),
                        span: Span::new(start, i),
                        newline_before: nl,
                    });
                } else {
                    out.push(Token {
                        kind: T::LParen,
                        span: Span::new(start, start + 1),
                        newline_before: nl,
                    });
                    let mut first = true;
                    for seg in segments {
                        if !first {
                            out.push(Token {
                                kind: T::Plus,
                                span: Span::new(start, i),
                                newline_before: false,
                            });
                        }
                        first = false;
                        match seg {
                            Seg::Str(s) => {
                                out.push(Token {
                                    kind: T::Str(s),
                                    span: Span::new(start, i),
                                    newline_before: false,
                                });
                            }
                            Seg::Expr(tokens) => {
                                out.push(Token {
                                    kind: T::LParen,
                                    span: Span::new(start, i),
                                    newline_before: false,
                                });
                                out.extend(tokens);
                                out.push(Token {
                                    kind: T::RParen,
                                    span: Span::new(start, i),
                                    newline_before: false,
                                });
                            }
                        }
                    }
                    out.push(Token {
                        kind: T::RParen,
                        span: Span::new(i - 1, i),
                        newline_before: false,
                    });
                }
                nl = false;
            }
            _ if is_ident_start(c) || is_ident_start_unicode(src[i..].chars().next().unwrap()) => {
                let start = i;
                while i < b.len() {
                    let ch = src[i..].chars().next().unwrap();
                    if is_ident_part(b[i]) || is_ident_part_unicode(ch) {
                        i += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                let text = &src[start..i];
                let kind = keyword(text).unwrap_or_else(|| T::Ident(text.to_string()));
                out.push(Token { kind, span: Span::new(start, i), newline_before: nl });
                nl = false;
            }
            _ => {
                let next = b.get(i + 1);
                let next2 = b.get(i + 2);
                let (kind, len) = match c {
                    b'(' => (T::LParen, 1),
                    b')' => (T::RParen, 1),
                    b'{' => (T::LBrace, 1),
                    b'}' => (T::RBrace, 1),
                    b'[' => (T::LBracket, 1),
                    b']' => (T::RBracket, 1),
                    b',' => (T::Comma, 1),
                    b';' => (T::Semi, 1),
                    b'.' => (T::Dot, 1),
                    b':' => (T::Colon, 1),
                    b'?' => match next.copied() {
                        Some(b'?') => (T::NullishCoalescing, 2),
                        Some(b'.') => (T::OptionalChain, 2),
                        _ => (T::Question, 1),
                    },
                    b'+' => match next.copied() {
                        Some(b'+') => (T::PlusPlus, 2),
                        Some(b'=') => (T::PlusEq, 2),
                        _ => (T::Plus, 1),
                    },
                    b'-' => match next.copied() {
                        Some(b'-') => (T::MinusMinus, 2),
                        Some(b'=') => (T::MinusEq, 2),
                        _ => (T::Minus, 1),
                    },
                    b'*' => {
                        if next == Some(&b'=') { (T::StarEq, 2) } else { (T::Star, 1) }
                    }
                    b'/' => {
                        if next == Some(&b'=') { (T::SlashEq, 2) } else { (T::Slash, 1) }
                    }
                    b'%' => {
                        if next == Some(&b'=') { (T::PercentEq, 2) } else { (T::Percent, 1) }
                    }
                    b'&' => {
                        if next == Some(&b'&') { (T::AmpAmp, 2) } else { (T::Amp, 1) }
                    }
                    b'|' => {
                        if next == Some(&b'|') { (T::PipePipe, 2) } else { (T::Pipe, 1) }
                    }
                    b'=' => {
                        if next == Some(&b'=') {
                            if next2 == Some(&b'=') { (T::EqEqEq, 3) } else { (T::EqEq, 2) }
                        } else if next == Some(&b'>') {
                            (T::FatArrow, 2)
                        } else {
                            (T::Assign, 1)
                        }
                    }
                    b'!' => {
                        if next == Some(&b'=') {
                            if next2 == Some(&b'=') { (T::BangEqEq, 3) } else { (T::BangEq, 2) }
                        } else {
                            (T::Bang, 1)
                        }
                    }
                    b'<' => {
                        if next == Some(&b'=') { (T::LtEq, 2) } else { (T::Lt, 1) }
                    }
                    b'>' => {
                        if next == Some(&b'=') { (T::GtEq, 2) } else { (T::Gt, 1) }
                    }
                    _ => {
                        let ch = src[i..].chars().next().unwrap();
                        return Err(Diagnostic::new(
                            format!("unexpected character '{ch}'"),
                            Span::new(i, i + ch.len_utf8()),
                        ));
                    }
                };
                out.push(Token { kind, span: Span::new(i, i + len), newline_before: nl });
                nl = false;
                i += len;
            }
        }
    }

    out.push(Token { kind: T::Eof, span: Span::new(src.len(), src.len()), newline_before: nl });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn simple_expression() {
        assert_eq!(
            kinds("1 + 2.5"),
            vec![TokenKind::Number(1.0), TokenKind::Plus, TokenKind::Number(2.5), TokenKind::Eof]
        );
    }

    #[test]
    fn longest_match_operators() {
        assert_eq!(
            kinds("a === b !== c <= d"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::EqEqEq,
                TokenKind::Ident("b".into()),
                TokenKind::BangEqEq,
                TokenKind::Ident("c".into()),
                TokenKind::LtEq,
                TokenKind::Ident("d".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn keywords_and_comments() {
        assert_eq!(
            kinds("// hi\nlet /* x */ x"),
            vec![TokenKind::Let, TokenKind::Ident("x".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn every_line_terminator_sets_newline_before() {
        for sep in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
            let t = tokenize(&format!("a{sep}b")).unwrap();
            assert!(t[1].newline_before, "{sep:?} must count as a line terminator");
        }
        let t = tokenize("a b").unwrap();
        assert!(!t[1].newline_before);
    }

    #[test]
    fn line_comments_end_at_any_line_terminator() {
        for sep in ["\n", "\r", "\u{2028}"] {
            let t = tokenize(&format!("a // note{sep}b")).unwrap();
            assert!(t[1].newline_before);
            assert!(matches!(&t[1].kind, TokenKind::Ident(n) if n == "b"), "{sep:?}");
        }
    }

    #[test]
    fn tracks_newlines() {
        let t = tokenize("a\nb c").unwrap();
        assert!(!t[0].newline_before);
        assert!(t[1].newline_before);
        assert!(!t[2].newline_before);
    }

    #[test]
    fn exponent_and_leading_dot() {
        assert_eq!(kinds("1e3 .5"), vec![TokenKind::Number(1000.0), TokenKind::Number(0.5), TokenKind::Eof]);
    }

    #[test]
    fn compound_and_logical_operators() {
        assert_eq!(
            kinds("a += b++ && c || d--"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::PlusEq,
                TokenKind::Ident("b".into()),
                TokenKind::PlusPlus,
                TokenKind::AmpAmp,
                TokenKind::Ident("c".into()),
                TokenKind::PipePipe,
                TokenKind::Ident("d".into()),
                TokenKind::MinusMinus,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn single_ampersand_is_tokenized() {
        assert_eq!(kinds("a & b"), vec![TokenKind::Ident("a".into()), TokenKind::Amp, TokenKind::Ident("b".into()), TokenKind::Eof]);
    }

    #[test]
    fn rejects_unknown_character() {
        let e = tokenize("let x = @;").unwrap_err();
        assert!(e.message.contains("unexpected character '@'"));
    }

    #[test]
    fn rejects_unterminated_comment() {
        assert!(tokenize("/* nope").is_err());
    }

    #[test]
    fn string_literals_and_escapes() {
        assert_eq!(
            kinds(r#""hello world" 'single' "escapes: \n\t\r\\\'\"" "\x41\u0042\u{43}""#),
            vec![
                TokenKind::Str("hello world".into()),
                TokenKind::Str("single".into()),
                TokenKind::Str("escapes: \n\t\r\\'\"".into()),
                TokenKind::Str("ABC".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn hex_binary_octal_and_separators() {
        assert_eq!(
            kinds("0xFF 0b1010 0o77 1_000_000 0x1_f"),
            vec![
                TokenKind::Number(255.0),
                TokenKind::Number(10.0),
                TokenKind::Number(63.0),
                TokenKind::Number(1000000.0),
                TokenKind::Number(31.0),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unicode_identifiers() {
        assert_eq!(
            kinds("let π = 3.14; let cafe_répertoire = 1;"),
            vec![
                TokenKind::Let,
                TokenKind::Ident("π".into()),
                TokenKind::Assign,
                TokenKind::Number(3.14),
                TokenKind::Semi,
                TokenKind::Let,
                TokenKind::Ident("cafe_répertoire".into()),
                TokenKind::Assign,
                TokenKind::Number(1.0),
                TokenKind::Semi,
                TokenKind::Eof,
            ]
        );
    }
}
