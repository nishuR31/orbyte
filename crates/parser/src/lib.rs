//! Recursive-descent parser with precedence climbing for expressions.
//!
//! Automatic semicolon insertion is approximated: a statement may end at `;`,
//! before `}`, at end of file, or when the next token starts on a new line.

use ast::*;
use diagnostics::{Diagnostic, Result, Span};
use lexer::{Token, TokenKind as T};

pub fn parse(source: &str) -> Result<Program> {
    let tokens = lexer::tokenize(source)?;
    let mut p = Parser { tokens, pos: 0 };
    let mut body = Vec::new();
    while !p.at(&T::Eof) {
        p.parse_stmt_into(&mut body)?;
    }
    Ok(Program { body })
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

fn binary_op(kind: &T) -> Option<(BinaryOp, u8)> {
    Some(match kind {
        T::PipePipe => (BinaryOp::Or, 1),
        T::AmpAmp => (BinaryOp::And, 2),
        T::NullishCoalescing => (BinaryOp::NullishCoalescing, 1),
        T::EqEq => (BinaryOp::Eq, 3),
        T::BangEq => (BinaryOp::NotEq, 3),
        T::EqEqEq => (BinaryOp::StrictEq, 3),
        T::BangEqEq => (BinaryOp::StrictNotEq, 3),
        T::Lt => (BinaryOp::Lt, 4),
        T::LtEq => (BinaryOp::LtEq, 4),
        T::Gt => (BinaryOp::Gt, 4),
        T::GtEq => (BinaryOp::GtEq, 4),
        T::Plus => (BinaryOp::Add, 5),
        T::Minus => (BinaryOp::Sub, 5),
        T::Star => (BinaryOp::Mul, 6),
        T::Slash => (BinaryOp::Div, 6),
        T::Percent => (BinaryOp::Rem, 6),
        _ => return None,
    })
}

fn compound_op(kind: &T) -> Option<BinaryOp> {
    Some(match kind {
        T::PlusEq => BinaryOp::Add,
        T::MinusEq => BinaryOp::Sub,
        T::StarEq => BinaryOp::Mul,
        T::SlashEq => BinaryOp::Div,
        T::PercentEq => BinaryOp::Rem,
        _ => return None,
    })
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn at(&self, kind: &T) -> bool {
        &self.peek().kind == kind
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn prev_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    fn eat(&mut self, kind: &T) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn error_expected(&self, what: &str) -> Diagnostic {
        let tok = self.peek();
        Diagnostic::new(
            format!("expected {what}, found {}", tok.kind.describe()),
            tok.span,
        )
    }

    fn expect(&mut self, kind: T, what: &str) -> Result<Token> {
        if self.at(&kind) {
            Ok(self.advance())
        } else {
            Err(self.error_expected(what))
        }
    }

    fn expect_ident(&mut self) -> Result<Ident> {
        if let T::Ident(name) = &self.peek().kind {
            let ident = Ident { name: name.clone(), span: self.peek().span };
            self.advance();
            Ok(ident)
        } else {
            Err(self.error_expected("an identifier"))
        }
    }

    fn consume_semicolon(&mut self) -> Result<()> {
        if self.eat(&T::Semi) {
            return Ok(());
        }
        if self.at(&T::RBrace) || self.at(&T::Eof) || self.peek().newline_before {
            return Ok(());
        }
        Err(self.error_expected("';'"))
    }

    // ---------- statements ----------

    /// Skips a TypeScript type annotation (`: <type>`), balancing any matched brackets/braces/parens/angles.
    fn skip_type_annotation(&mut self) -> Result<()> {
        if !self.eat(&T::Colon) {
            return Ok(());
        }
        let mut depth_paren = 0usize;
        let mut depth_brace = 0usize;
        let mut depth_bracket = 0usize;
        let mut depth_angle = 0usize;

        while !self.at(&T::Eof) {
            let tok = self.peek();
            if depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 && depth_angle == 0 {
                // A type annotation ends at assignment '=', comma ',', closing paren ')', semicolon ';', right brace '}', or opening brace '{'
                if matches!(tok.kind, T::Assign | T::Comma | T::RParen | T::Semi | T::RBrace | T::LBrace) || tok.newline_before {
                    break;
                }
            }

            match &tok.kind {
                T::LParen => depth_paren += 1,
                T::RParen if depth_paren > 0 => depth_paren -= 1,
                T::LBrace => depth_brace += 1,
                T::RBrace if depth_brace > 0 => depth_brace -= 1,
                T::LBracket => depth_bracket += 1,
                T::RBracket if depth_bracket > 0 => depth_bracket -= 1,
                T::Lt => depth_angle += 1,
                T::Gt if depth_angle > 0 => depth_angle -= 1,
                _ => {}
            }
            self.advance();
        }
        Ok(())
    }

    /// Skips a TypeScript type alias declaration (`type Foo = ...;`) or interface declaration (`interface Foo { ... }`).
    fn skip_type_decl(&mut self) -> Result<Stmt> {
        let start = self.advance(); // consume 'type' or 'interface'
        let is_interface = start.kind == T::Interface;
        let mut depth_brace = 0usize;
        while !self.at(&T::Eof) {
            let tok = self.peek();
            if tok.kind == T::LBrace {
                depth_brace += 1;
            } else if tok.kind == T::RBrace {
                if depth_brace > 0 {
                    depth_brace -= 1;
                    if depth_brace == 0 && is_interface {
                        // Interface ends at matching '}'
                        self.advance();
                        let _ = self.eat(&T::Semi);
                        break;
                    }
                }
            } else if depth_brace == 0 {
                if tok.kind == T::Semi {
                    self.advance();
                    break;
                }
                if tok.newline_before {
                    break;
                }
            }
            self.advance();
        }
        Ok(Stmt { kind: StmtKind::Empty, span: start.span })
    }

    /// Skips TypeScript/ES module header (`export ...` or `import ...`).
    fn skip_module_decl(&mut self) -> Result<Stmt> {
        let start = self.advance(); // consume 'export' or 'import'
        if start.kind == T::Import {
            // `import(...)` and `import.meta` are expressions, not module
            // declarations. The native pipeline has no module system, so reject
            // them rather than silently dropping a statement that should have
            // executed (or resolved a module).
            match self.peek().kind {
                T::LParen => {
                    return Err(Diagnostic::new(
                        "dynamic import is not supported by the native pipeline",
                        start.span.join(self.peek().span),
                    ));
                }
                T::Dot => {
                    return Err(Diagnostic::new(
                        "import.meta is not supported by the native pipeline",
                        start.span.join(self.peek().span),
                    ));
                }
                _ => {}
            }
        }
        if start.kind == T::Export {
            // 'export const x = 1;' -> parse the inner statement ('const x = 1;')
            if matches!(self.peek().kind, T::Let | T::Const | T::Var | T::Function) {
                return self.parse_stmt();
            }
            if matches!(self.peek().kind, T::Type | T::Interface) {
                return self.skip_type_decl();
            }
        }
        // General export / import statement ending with semicolon
        let mut depth_brace = 0usize;
        while !self.at(&T::Eof) {
            let tok = self.peek();
            if tok.kind == T::LBrace {
                depth_brace += 1;
            } else if tok.kind == T::RBrace {
                if depth_brace > 0 {
                    depth_brace -= 1;
                }
            } else if depth_brace == 0 && (tok.kind == T::Semi || tok.newline_before) {
                let _ = self.eat(&T::Semi);
                break;
            }
            self.advance();
        }
        Ok(Stmt { kind: StmtKind::Empty, span: start.span })
    }

    fn parse_stmt_into(&mut self, out: &mut Vec<Stmt>) -> Result<()> {
        if matches!(self.peek().kind, T::Let | T::Const | T::Var) {
            let start = self.advance();
            let kind = match start.kind {
                T::Let => VarKind::Let,
                T::Const => VarKind::Const,
                _ => VarKind::Var,
            };
            loop {
                if self.eat(&T::LBracket) {
                    let mut names = Vec::new();
                    while !self.at(&T::RBracket) && !self.at(&T::Eof) {
                        names.push(self.expect_ident()?);
                        if !self.eat(&T::Comma) {
                            break;
                        }
                    }
                    self.expect(T::RBracket, "']'")?;
                    self.skip_type_annotation()?;
                    self.expect(T::Assign, "'='")?;
                    let init_expr = self.parse_expr()?;
                    let tmp_name = format!("__destruct_{}", self.pos);
                    let tmp_ident = Ident { name: tmp_name.clone(), span: start.span };
                    out.push(Stmt {
                        kind: StmtKind::Var {
                            kind,
                            name: tmp_ident,
                            init: Some(init_expr),
                        },
                        span: start.span.join(self.prev_span()),
                    });
                    for (i, n) in names.into_iter().enumerate() {
                        let elem_expr = Expr {
                            kind: ExprKind::Index {
                                object: Box::new(Expr {
                                    kind: ExprKind::Ident(tmp_name.clone()),
                                    span: n.span,
                                }),
                                index: Box::new(Expr {
                                    kind: ExprKind::Number(i as f64),
                                    span: n.span,
                                }),
                            },
                            span: n.span,
                        };
                        out.push(Stmt {
                            kind: StmtKind::Var {
                                kind,
                                name: n.clone(),
                                init: Some(elem_expr),
                            },
                            span: n.span,
                        });
                    }
                } else if self.eat(&T::LBrace) {
                    let mut names = Vec::new();
                    while !self.at(&T::RBrace) && !self.at(&T::Eof) {
                        names.push(self.expect_ident()?);
                        if !self.eat(&T::Comma) {
                            break;
                        }
                    }
                    self.expect(T::RBrace, "'}'")?;
                    self.skip_type_annotation()?;
                    self.expect(T::Assign, "'='")?;
                    let init_expr = self.parse_expr()?;
                    let tmp_name = format!("__destruct_obj_{}", self.pos);
                    let tmp_ident = Ident { name: tmp_name.clone(), span: start.span };
                    out.push(Stmt {
                        kind: StmtKind::Var {
                            kind,
                            name: tmp_ident,
                            init: Some(init_expr),
                        },
                        span: start.span.join(self.prev_span()),
                    });
                    for n in names {
                        let prop_expr = Expr {
                            kind: ExprKind::Member {
                                object: Box::new(Expr {
                                    kind: ExprKind::Ident(tmp_name.clone()),
                                    span: n.span,
                                }),
                                property: n.clone(),
                            },
                            span: n.span,
                        };
                        out.push(Stmt {
                            kind: StmtKind::Var {
                                kind,
                                name: n.clone(),
                                init: Some(prop_expr),
                            },
                            span: n.span,
                        });
                    }
                } else {
                    let name = self.expect_ident()?;
                    self.skip_type_annotation()?;
                    if self.eat(&T::Assign) {
                        let is_paren_arrow = if self.at(&T::LParen) {
                            let mut depth = 0;
                            let mut found = false;
                            for i in self.pos..self.tokens.len() {
                                match &self.tokens[i].kind {
                                    T::LParen => depth += 1,
                                    T::RParen => {
                                        depth -= 1;
                                        if depth == 0 {
                                            if let Some(next) = self.tokens.get(i + 1) {
                                                if next.kind == T::FatArrow {
                                                    found = true;
                                                }
                                            }
                                            break;
                                        }
                                    }
                                    T::Semi | T::Eof => break,
                                    _ => {}
                                }
                            }
                            found
                        } else {
                            false
                        };
                        let is_ident_arrow = matches!(self.peek().kind, T::Ident(_))
                            && self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&T::FatArrow);
                        let is_filter_call = matches!(self.peek().kind, T::Ident(_))
                            && self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&T::Dot)
                            && matches!(self.tokens.get(self.pos + 2).map(|t| &t.kind), Some(T::Ident(s)) if s == "filter")
                            && self.tokens.get(self.pos + 3).map(|t| &t.kind) == Some(&T::LParen);

                        if is_paren_arrow {
                            self.advance(); // consume '('
                            let mut params = Vec::new();
                            while !self.at(&T::RParen) && !self.at(&T::Eof) {
                                params.push(self.expect_ident()?);
                                if !self.eat(&T::Comma) {
                                    break;
                                }
                            }
                            self.expect(T::RParen, "')'")?;
                            self.expect(T::FatArrow, "'=>'")?;
                            let body = if self.at(&T::LBrace) {
                                let (b, _) = self.parse_block_body()?;
                                b
                            } else {
                                let ret_expr = self.parse_expr()?;
                                vec![Stmt {
                                    kind: StmtKind::Return(Some(ret_expr)),
                                    span: start.span,
                                }]
                            };
                            out.push(Stmt {
                                kind: StmtKind::Function(FunctionDecl {
                                    name,
                                    params,
                                    body,
                                    span: start.span.join(self.prev_span()),
                                }),
                                span: start.span.join(self.prev_span()),
                            });
                        } else if is_ident_arrow {
                            let param = self.expect_ident()?;
                            self.expect(T::FatArrow, "'=>'")?;
                            let body = if self.at(&T::LBrace) {
                                let (b, _) = self.parse_block_body()?;
                                b
                            } else {
                                let ret_expr = self.parse_expr()?;
                                vec![Stmt {
                                    kind: StmtKind::Return(Some(ret_expr)),
                                    span: start.span,
                                }]
                            };
                            out.push(Stmt {
                                kind: StmtKind::Function(FunctionDecl {
                                    name,
                                    params: vec![param],
                                    body,
                                    span: start.span.join(self.prev_span()),
                                }),
                                span: start.span.join(self.prev_span()),
                            });
                        } else if is_filter_call {
                            let arr_ident = self.expect_ident()?;
                            self.expect(T::Dot, "'.'")?;
                            self.advance(); // consume 'filter'
                            self.expect(T::LParen, "'('")?;
                            let param = if self.eat(&T::LParen) {
                                let p = self.expect_ident()?;
                                self.expect(T::RParen, "')'")?;
                                p
                            } else {
                                self.expect_ident()?
                            };
                            self.expect(T::FatArrow, "'=>'")?;
                            let pred = self.parse_expr()?;
                            self.expect(T::RParen, "')'")?;

                            out.push(Stmt {
                                kind: StmtKind::Var {
                                    kind,
                                    name: name.clone(),
                                    init: Some(Expr {
                                        kind: ExprKind::Array(Vec::new()),
                                        span: start.span,
                                    }),
                                },
                                span: start.span,
                            });
                            let idx_name = format!("__filter_idx_{}", self.pos);
                            let idx_ident = Ident { name: idx_name.clone(), span: start.span };
                            let init_stmt = Stmt {
                                kind: StmtKind::Var {
                                    kind: VarKind::Let,
                                    name: idx_ident.clone(),
                                    init: Some(Expr { kind: ExprKind::Number(0.0), span: start.span }),
                                },
                                span: start.span,
                            };
                            let cond_expr = Expr {
                                kind: ExprKind::Binary {
                                    op: BinaryOp::Lt,
                                    lhs: Box::new(Expr { kind: ExprKind::Ident(idx_name.clone()), span: start.span }),
                                    rhs: Box::new(Expr {
                                        kind: ExprKind::Member {
                                            object: Box::new(Expr { kind: ExprKind::Ident(arr_ident.name.clone()), span: start.span }),
                                            property: Ident { name: "length".to_string(), span: start.span },
                                        },
                                        span: start.span,
                                    }),
                                },
                                span: start.span,
                            };
                            let update_expr = Expr {
                                kind: ExprKind::Update {
                                    target: idx_ident.clone(),
                                    increment: true,
                                    prefix: false,
                                },
                                span: start.span,
                            };
                            let elem_var = Stmt {
                                kind: StmtKind::Var {
                                    kind: VarKind::Let,
                                    name: param.clone(),
                                    init: Some(Expr {
                                        kind: ExprKind::Index {
                                            object: Box::new(Expr { kind: ExprKind::Ident(arr_ident.name.clone()), span: start.span }),
                                            index: Box::new(Expr { kind: ExprKind::Ident(idx_name.clone()), span: start.span }),
                                        },
                                        span: start.span,
                                    }),
                                },
                                span: start.span,
                            };
                            let push_stmt = Stmt {
                                kind: StmtKind::Expr(Expr {
                                    kind: ExprKind::Call {
                                        callee: Box::new(Expr {
                                            kind: ExprKind::Member {
                                                object: Box::new(Expr { kind: ExprKind::Ident(name.name.clone()), span: start.span }),
                                                property: Ident { name: "push".to_string(), span: start.span },
                                            },
                                            span: start.span,
                                        }),
                                        args: vec![Expr { kind: ExprKind::Ident(param.name.clone()), span: start.span }],
                                    },
                                    span: start.span,
                                }),
                                span: start.span,
                            };
                            let if_stmt = Stmt {
                                kind: StmtKind::If {
                                    cond: pred,
                                    then: Box::new(push_stmt),
                                    otherwise: None,
                                },
                                span: start.span,
                            };
                            let loop_body = Stmt {
                                kind: StmtKind::Block(vec![elem_var, if_stmt]),
                                span: start.span,
                            };
                            out.push(Stmt {
                                kind: StmtKind::For {
                                    init: Some(Box::new(init_stmt)),
                                    cond: Some(cond_expr),
                                    update: Some(update_expr),
                                    body: Box::new(loop_body),
                                },
                                span: start.span,
                            });
                        } else {
                            let init = Some(self.parse_expr()?);
                            out.push(Stmt {
                                kind: StmtKind::Var { kind, name, init },
                                span: start.span.join(self.prev_span()),
                            });
                        }
                    } else {
                        out.push(Stmt {
                            kind: StmtKind::Var { kind, name, init: None },
                            span: start.span.join(self.prev_span()),
                        });
                    }
                }
                if !self.eat(&T::Comma) {
                    break;
                }
            }
            self.consume_semicolon()?;
            return Ok(());
        }
        out.push(self.parse_stmt()?);
        Ok(())
    }

    fn parse_stmt(&mut self) -> Result<Stmt> {
        let start = self.peek().span;
        let kind = self.peek().kind.clone();
        match kind {
            T::Type | T::Interface => self.skip_type_decl(),
            T::Export | T::Import => self.skip_module_decl(),
            T::Function => {
                let f = self.parse_function()?;
                let span = f.span;
                Ok(Stmt { kind: StmtKind::Function(f), span })
            }
            T::Let | T::Const | T::Var => self.parse_var_decl(),
            T::If => {
                self.advance();
                self.expect(T::LParen, "'('")?;
                let cond = self.parse_expr()?;
                self.expect(T::RParen, "')'")?;
                let then = Box::new(self.parse_stmt()?);
                let otherwise = if self.eat(&T::Else) {
                    Some(Box::new(self.parse_stmt()?))
                } else {
                    None
                };
                let span = start.join(self.prev_span());
                Ok(Stmt { kind: StmtKind::If { cond, then, otherwise }, span })
            }
            T::While => {
                self.advance();
                self.expect(T::LParen, "'('")?;
                let cond = self.parse_expr()?;
                self.expect(T::RParen, "')'")?;
                let body = Box::new(self.parse_stmt()?);
                let span = start.join(self.prev_span());
                Ok(Stmt { kind: StmtKind::While { cond, body }, span })
            }
            T::For => self.parse_for(),
            T::Break | T::Continue => {
                self.advance();
                self.consume_semicolon()?;
                let span = start.join(self.prev_span());
                let kind = if kind == T::Break { StmtKind::Break } else { StmtKind::Continue };
                Ok(Stmt { kind, span })
            }
            T::Return => {
                self.advance();
                let value = if self.at(&T::Semi)
                    || self.at(&T::RBrace)
                    || self.at(&T::Eof)
                    || self.peek().newline_before
                {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                self.consume_semicolon()?;
                let span = start.join(self.prev_span());
                Ok(Stmt { kind: StmtKind::Return(value), span })
            }
            T::LBrace => {
                let (body, span) = self.parse_block_body()?;
                Ok(Stmt { kind: StmtKind::Block(body), span })
            }
            T::Semi => {
                self.advance();
                Ok(Stmt { kind: StmtKind::Empty, span: start })
            }
            _ => {
                let expr = self.parse_expr()?;
                self.consume_semicolon()?;
                let span = start.join(self.prev_span());
                Ok(Stmt { kind: StmtKind::Expr(expr), span })
            }
        }
    }

    fn parse_for(&mut self) -> Result<Stmt> {
        let start = self.expect(T::For, "'for'")?.span;
        self.expect(T::LParen, "'('")?;
        let init = if self.at(&T::Semi) {
            self.advance();
            None
        } else if matches!(self.peek().kind, T::Let | T::Const | T::Var) {
            Some(Box::new(self.parse_var_decl_in(true)?)) // consumes its own ';'
        } else {
            let first = self.peek().span;
            let e = self.parse_expr()?;
            self.expect(T::Semi, "';'")?;
            let span = first.join(self.prev_span());
            Some(Box::new(Stmt { kind: StmtKind::Expr(e), span }))
        };
        let cond = if self.at(&T::Semi) { None } else { Some(self.parse_expr()?) };
        self.expect(T::Semi, "';'")?;
        let update = if self.at(&T::RParen) { None } else { Some(self.parse_expr()?) };
        self.expect(T::RParen, "')'")?;
        let body = Box::new(self.parse_stmt()?);
        let span = start.join(self.prev_span());
        Ok(Stmt { kind: StmtKind::For { init, cond, update, body }, span })
    }

    fn parse_block_body(&mut self) -> Result<(Vec<Stmt>, Span)> {
        let open = self.expect(T::LBrace, "'{'")?;
        let mut body = Vec::new();
        loop {
            if self.at(&T::Eof) {
                return Err(self.error_expected("'}'"));
            }
            if self.at(&T::RBrace) {
                break;
            }
            self.parse_stmt_into(&mut body)?;
        }
        let close = self.advance();
        Ok((body, open.span.join(close.span)))
    }

    fn parse_function(&mut self) -> Result<FunctionDecl> {
        let start = self.expect(T::Function, "'function'")?.span;
        let name = self.expect_ident()?;
        self.expect(T::LParen, "'('")?;
        let mut params = Vec::new();
        let mut param_prologue = Vec::new();
        if !self.at(&T::RParen) {
            loop {
                if self.eat(&T::LBrace) {
                    let mut names = Vec::new();
                    while !self.at(&T::RBrace) && !self.at(&T::Eof) {
                        names.push(self.expect_ident()?);
                        if !self.eat(&T::Comma) {
                            break;
                        }
                    }
                    self.expect(T::RBrace, "'}'")?;
                    self.skip_type_annotation()?;
                    let p_name = format!("__param_obj_{}", params.len());
                    let p_ident = Ident { name: p_name.clone(), span: start };
                    params.push(p_ident);
                    for n in names {
                        let prop_expr = Expr {
                            kind: ExprKind::Member {
                                object: Box::new(Expr {
                                    kind: ExprKind::Ident(p_name.clone()),
                                    span: n.span,
                                }),
                                property: n.clone(),
                            },
                            span: n.span,
                        };
                        param_prologue.push(Stmt {
                            kind: StmtKind::Var {
                                kind: VarKind::Let,
                                name: n.clone(),
                                init: Some(prop_expr),
                            },
                            span: n.span,
                        });
                    }
                } else if self.eat(&T::LBracket) {
                    let mut names = Vec::new();
                    while !self.at(&T::RBracket) && !self.at(&T::Eof) {
                        names.push(self.expect_ident()?);
                        if !self.eat(&T::Comma) {
                            break;
                        }
                    }
                    self.expect(T::RBracket, "']'")?;
                    self.skip_type_annotation()?;
                    let p_name = format!("__param_arr_{}", params.len());
                    let p_ident = Ident { name: p_name.clone(), span: start };
                    params.push(p_ident);
                    for (i, n) in names.into_iter().enumerate() {
                        let elem_expr = Expr {
                            kind: ExprKind::Index {
                                object: Box::new(Expr {
                                    kind: ExprKind::Ident(p_name.clone()),
                                    span: n.span,
                                }),
                                index: Box::new(Expr {
                                    kind: ExprKind::Number(i as f64),
                                    span: n.span,
                                }),
                            },
                            span: n.span,
                        };
                        param_prologue.push(Stmt {
                            kind: StmtKind::Var {
                                kind: VarKind::Let,
                                name: n.clone(),
                                init: Some(elem_expr),
                            },
                            span: n.span,
                        });
                    }
                } else {
                    let p = self.expect_ident()?;
                    self.eat(&T::Question); // skip optional `?` marker (TypeScript)
                    self.skip_type_annotation()?;
                    params.push(p);
                }
                if !self.eat(&T::Comma) {
                    break;
                }
            }
        }
        self.expect(T::RParen, "')'")?;
        self.skip_type_annotation()?; // skip function return type `: number`
        let (mut body, end) = self.parse_block_body()?;
        if !param_prologue.is_empty() {
            param_prologue.extend(body);
            body = param_prologue;
        }
        Ok(FunctionDecl { name, params, body, span: start.join(end) })
    }

    fn parse_var_decl(&mut self) -> Result<Stmt> {
        self.parse_var_decl_in(false)
    }

    /// `in_for_header` declarations must end in a real `;`: automatic semicolon
    /// insertion never applies inside the parentheses of a `for` statement.
    fn parse_var_decl_in(&mut self, in_for_header: bool) -> Result<Stmt> {
        let start = self.advance();
        let kind = match start.kind {
            T::Let => VarKind::Let,
            T::Const => VarKind::Const,
            _ => VarKind::Var,
        };
        let name = self.expect_ident()?;
        self.skip_type_annotation()?; // skip variable type annotation `: number`
        let init = if self.eat(&T::Assign) { Some(self.parse_expr()?) } else { None };
        if in_for_header {
            self.expect(T::Semi, "';'")?;
        } else {
            self.consume_semicolon()?;
        }
        let span = start.span.join(self.prev_span());
        Ok(Stmt { kind: StmtKind::Var { kind, name, init }, span })
    }

    // ---------- expressions ----------

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<Expr> {
        let lhs = self.parse_binary(1)?;
        if let Some(op) = compound_op(&self.peek().kind) {
            let ExprKind::Ident(name) = &lhs.kind else {
                return Err(Diagnostic::new("invalid assignment target", lhs.span));
            };
            let target = Ident { name: name.clone(), span: lhs.span };
            self.advance();
            let rhs = self.parse_assignment()?;
            let span = lhs.span.join(rhs.span);
            // `x op= y` is `x = x op y` (the target is a plain identifier, so
            // evaluating it twice has no side effects).
            let combined = Expr {
                kind: ExprKind::Binary { op, lhs: Box::new(lhs.clone()), rhs: Box::new(rhs) },
                span,
            };
            return Ok(Expr {
                kind: ExprKind::Assign { target, value: Box::new(combined) },
                span,
            });
        }
        if self.at(&T::Assign) {
            match lhs.kind {
                ExprKind::Ident(name) => {
                    let target = Ident { name, span: lhs.span };
                    self.advance();
                    let value = self.parse_assignment()?;
                    let span = lhs.span.join(value.span);
                    return Ok(Expr {
                        kind: ExprKind::Assign { target, value: Box::new(value) },
                        span,
                    });
                }
                ExprKind::Member { object, property } => {
                    self.advance();
                    let value = self.parse_assignment()?;
                    let span = lhs.span.join(value.span);
                    return Ok(Expr {
                        kind: ExprKind::MemberAssign { object, property, value: Box::new(value) },
                        span,
                    });
                }
                ExprKind::Index { object, index } => {
                    self.advance();
                    let value = self.parse_assignment()?;
                    let span = lhs.span.join(value.span);
                    return Ok(Expr {
                        kind: ExprKind::IndexAssign { object, index, value: Box::new(value) },
                        span,
                    });
                }
                _ => return Err(Diagnostic::new("invalid assignment target", lhs.span)),
            }
        }
        Ok(lhs)
    }

    fn parse_binary(&mut self, min_prec: u8) -> Result<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            let Some((op, prec)) = binary_op(&self.peek().kind) else { break };
            if prec < min_prec {
                break;
            }
            self.advance();
            let rhs = self.parse_binary(prec + 1)?;
            let span = lhs.span.join(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) },
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        let tok = self.peek().clone();
        if matches!(tok.kind, T::PlusPlus | T::MinusMinus) {
            self.advance();
            let target = self.expect_ident()?;
            let span = tok.span.join(target.span);
            return Ok(Expr {
                kind: ExprKind::Update { target, increment: tok.kind == T::PlusPlus, prefix: true },
                span,
            });
        }
        let op = match tok.kind {
            T::Minus => Some(UnaryOp::Neg),
            T::Bang => Some(UnaryOp::Not),
            T::Typeof => Some(UnaryOp::TypeOf),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let operand = self.parse_unary()?;
            let span = tok.span.join(operand.span);
            return Ok(Expr {
                kind: ExprKind::Unary { op, operand: Box::new(operand) },
                span,
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut e = self.parse_primary()?;
        loop {
            if self.at(&T::LParen) {
                self.advance();
                let mut args = Vec::new();
                if !self.at(&T::RParen) {
                    loop {
                        args.push(self.parse_expr()?);
                        if !self.eat(&T::Comma) {
                            break;
                        }
                    }
                }
                let close = self.expect(T::RParen, "')'")?;
                let span = e.span.join(close.span);
                e = Expr { kind: ExprKind::Call { callee: Box::new(e), args }, span };
            } else if self.at(&T::Dot) {
                self.advance();
                let property = self.expect_ident()?;
                let span = e.span.join(property.span);
                e = Expr { kind: ExprKind::Member { object: Box::new(e), property }, span };
            } else if self.at(&T::OptionalChain) {
                self.advance();
                let property = self.expect_ident()?;
                let span = e.span.join(property.span);
                e = Expr { kind: ExprKind::OptionalChain { object: Box::new(e), property }, span };
            } else if self.at(&T::LBracket) {
                self.advance();
                let index = self.parse_expr()?;
                let close = self.expect(T::RBracket, "']'")?;
                let span = e.span.join(close.span);
                e = Expr { kind: ExprKind::Index { object: Box::new(e), index: Box::new(index) }, span };
            } else if matches!(self.peek().kind, T::PlusPlus | T::MinusMinus)
                && !self.peek().newline_before
            {
                let ExprKind::Ident(name) = &e.kind else {
                    return Err(Diagnostic::new("invalid operand for '++' or '--'", e.span));
                };
                let target = Ident { name: name.clone(), span: e.span };
                let op = self.advance();
                let span = e.span.join(op.span);
                e = Expr {
                    kind: ExprKind::Update {
                        target,
                        increment: op.kind == T::PlusPlus,
                        prefix: false,
                    },
                    span,
                };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        let tok = self.peek().clone();
        match &tok.kind {
            T::Number(n) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Number(*n), span: tok.span })
            }
            T::Str(s) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Str(s.clone()), span: tok.span })
            }
            T::True => {
                self.advance();
                Ok(Expr { kind: ExprKind::Bool(true), span: tok.span })
            }
            T::False => {
                self.advance();
                Ok(Expr { kind: ExprKind::Bool(false), span: tok.span })
            }
            T::Null => {
                self.advance();
                Ok(Expr { kind: ExprKind::Null, span: tok.span })
            }
            T::Undefined => {
                self.advance();
                Ok(Expr { kind: ExprKind::Undefined, span: tok.span })
            }
            T::Ident(name) => {
                self.advance();
                Ok(Expr { kind: ExprKind::Ident(name.clone()), span: tok.span })
            }
            T::LParen => {
                self.advance();
                let inner = self.parse_expr()?;
                self.expect(T::RParen, "')'")?;
                Ok(inner)
            }
            T::LBracket => {
                let start = self.advance();
                let mut elements = Vec::new();
                if !self.at(&T::RBracket) {
                    loop {
                        elements.push(self.parse_expr()?);
                        if !self.eat(&T::Comma) {
                            break;
                        }
                    }
                }
                let close = self.expect(T::RBracket, "']'")?;
                Ok(Expr { kind: ExprKind::Array(elements), span: start.span.join(close.span) })
            }
            T::LBrace => {
                let start = self.advance();
                let mut properties = Vec::new();
                if !self.at(&T::RBrace) {
                    loop {
                        let key = self.expect_ident()?;
                        self.expect(T::Colon, "':'")?;
                        let val = self.parse_expr()?;
                        properties.push((key, val));
                        if !self.eat(&T::Comma) {
                            break;
                        }
                    }
                }
                let close = self.expect(T::RBrace, "'}'")?;
                Ok(Expr { kind: ExprKind::Object(properties), span: start.span.join(close.span) })
            }
            other => Err(Diagnostic::new(
                format!("expected an expression, found {}", other.describe()),
                tok.span,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_milestone_example() {
        let p = parse(
            "function calculate(a, b) { return a * b + 10; }\nconsole.log(calculate(20, 30));",
        )
        .unwrap();
        assert_eq!(p.body.len(), 2);
        assert!(matches!(&p.body[0].kind, StmtKind::Function(f) if f.params.len() == 2));
        assert!(matches!(
            &p.body[1].kind,
            StmtKind::Expr(Expr { kind: ExprKind::Call { .. }, .. })
        ));
    }

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        let p = parse("1 + 2 * 3;").unwrap();
        match &p.body[0].kind {
            StmtKind::Expr(Expr { kind: ExprKind::Binary { op: BinaryOp::Add, rhs, .. }, .. }) => {
                assert!(matches!(rhs.kind, ExprKind::Binary { op: BinaryOp::Mul, .. }));
            }
            other => panic!("unexpected tree: {other:?}"),
        }
    }

    #[test]
    fn subtraction_is_left_associative() {
        let p = parse("1 - 2 - 3;").unwrap();
        match &p.body[0].kind {
            StmtKind::Expr(Expr { kind: ExprKind::Binary { op: BinaryOp::Sub, lhs, .. }, .. }) => {
                assert!(matches!(lhs.kind, ExprKind::Binary { op: BinaryOp::Sub, .. }));
            }
            other => panic!("unexpected tree: {other:?}"),
        }
    }

    #[test]
    fn assignment_is_right_associative() {
        let p = parse("a = b = 1;").unwrap();
        match &p.body[0].kind {
            StmtKind::Expr(Expr { kind: ExprKind::Assign { value, .. }, .. }) => {
                assert!(matches!(value.kind, ExprKind::Assign { .. }));
            }
            other => panic!("unexpected tree: {other:?}"),
        }
    }

    #[test]
    fn semicolon_insertion_on_newlines() {
        let p = parse("let a = 1\nlet b = 2\n").unwrap();
        assert_eq!(p.body.len(), 2);
    }

    #[test]
    fn return_followed_by_newline_has_no_value() {
        let p = parse("function f() { return\n5 }").unwrap();
        match &p.body[0].kind {
            StmtKind::Function(f) => assert!(matches!(f.body[0].kind, StmtKind::Return(None))),
            other => panic!("unexpected tree: {other:?}"),
        }
    }

    #[test]
    fn logical_operators_have_lowest_binary_precedence() {
        let p = parse("a == 1 || b < 2 && c;").unwrap();
        match &p.body[0].kind {
            StmtKind::Expr(Expr { kind: ExprKind::Binary { op: BinaryOp::Or, rhs, .. }, .. }) => {
                assert!(matches!(rhs.kind, ExprKind::Binary { op: BinaryOp::And, .. }));
            }
            other => panic!("unexpected tree: {other:?}"),
        }
    }

    #[test]
    fn compound_assignment_desugars() {
        let p = parse("x += 2;").unwrap();
        match &p.body[0].kind {
            StmtKind::Expr(Expr { kind: ExprKind::Assign { value, .. }, .. }) => {
                assert!(matches!(value.kind, ExprKind::Binary { op: BinaryOp::Add, .. }));
            }
            other => panic!("unexpected tree: {other:?}"),
        }
    }

    #[test]
    fn parses_for_break_continue_and_updates() {
        let p = parse("for (let i = 0; i < 3; i++) { if (i == 1) continue; break; }").unwrap();
        match &p.body[0].kind {
            StmtKind::For { init, cond, update, .. } => {
                assert!(init.is_some() && cond.is_some());
                assert!(matches!(
                    update,
                    Some(Expr { kind: ExprKind::Update { increment: true, prefix: false, .. }, .. })
                ));
            }
            other => panic!("unexpected tree: {other:?}"),
        }
        assert!(parse("for (;;) { break }").is_ok());
    }

    #[test]
    fn expressions_continue_across_newlines() {
        // A newline does not end a statement when the next token continues it.
        let p = parse("let x = 10\n  + 20\nlet y = 1").unwrap();
        assert_eq!(p.body.len(), 2);
        assert!(matches!(
            &p.body[0].kind,
            StmtKind::Var { init: Some(Expr { kind: ExprKind::Binary { op: BinaryOp::Add, .. }, .. }), .. }
        ));
    }

    #[test]
    fn statements_split_on_every_line_terminator() {
        for sep in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
            let p = parse(&format!("let a = 1{sep}let b = 2")).unwrap();
            assert_eq!(p.body.len(), 2, "{sep:?}");
        }
    }

    #[test]
    fn no_semicolon_insertion_inside_for_headers() {
        assert!(parse("for (let i = 0\n i < 3; i++) { }").is_err());
        assert!(parse("for (let i = 0; i < 3\n i++) { }").is_err());
        assert!(parse("for (let i = 0;\n i < 3;\n i++) { }").is_ok());
    }

    #[test]
    fn postfix_increment_respects_the_line_terminator_restriction() {
        // `a \n ++b` is `a; ++b`, not `a++; b`.
        let p = parse("a\n++b").unwrap();
        assert_eq!(p.body.len(), 2);
        assert!(matches!(
            &p.body[1].kind,
            StmtKind::Expr(Expr { kind: ExprKind::Update { prefix: true, .. }, .. })
        ));
    }

    #[test]
    fn break_and_continue_end_at_a_line_terminator() {
        let p = parse("while (true) { break\nlet a = 1 }").unwrap();
        assert!(matches!(&p.body[0].kind, StmtKind::While { .. }));
    }

    #[test]
    fn rejects_increment_of_non_identifier() {
        assert!(parse("1++;").is_err());
    }

    #[test]
    fn reports_missing_paren() {
        let e = parse("console.log(1;").unwrap_err();
        assert!(e.message.contains("expected ')'"), "{}", e.message);
    }

    #[test]
    fn rejects_invalid_assignment_target() {
        assert!(parse("1 = 2;").is_err());
    }

    // ---- TypeScript type erasure tests ----

    #[test]
    fn ts_variable_type_annotation_is_stripped() {
        let p = parse("let x: number = 42;").unwrap();
        assert_eq!(p.body.len(), 1);
        assert!(matches!(&p.body[0].kind, StmtKind::Var { init: Some(_), .. }));
    }

    #[test]
    fn ts_function_param_and_return_types_are_stripped() {
        let p = parse("function add(a: number, b: number): number { return a + b; }").unwrap();
        assert_eq!(p.body.len(), 1);
        assert!(matches!(&p.body[0].kind, StmtKind::Function(f) if f.params.len() == 2));
    }

    #[test]
    fn ts_type_alias_is_skipped() {
        let p = parse("type BigUnion = { name: '0' } | { name: '1' };").unwrap();
        assert_eq!(p.body.len(), 1);
        assert!(matches!(&p.body[0].kind, StmtKind::Empty));
    }

    #[test]
    fn ts_interface_is_skipped() {
        let p = parse("interface User { id: number; name: string; }").unwrap();
        assert_eq!(p.body.len(), 1);
        assert!(matches!(&p.body[0].kind, StmtKind::Empty));
    }

    #[test]
    fn ts_export_const_is_parsed() {
        let p = parse("export const foo: string = \"hello\";").unwrap();
        assert_eq!(p.body.len(), 1);
        assert!(matches!(&p.body[0].kind, StmtKind::Var { .. }));
    }

    #[test]
    fn ts_export_function_is_parsed() {
        let p = parse("export function greet(name: string): string { return name; }").unwrap();
        assert_eq!(p.body.len(), 1);
        assert!(matches!(&p.body[0].kind, StmtKind::Function(_)));
    }

    #[test]
    fn ts_import_statement_is_skipped() {
        let p = parse("import { foo } from \"./bar\";").unwrap();
        assert_eq!(p.body.len(), 1);
        assert!(matches!(&p.body[0].kind, StmtKind::Empty));
    }

    #[test]
    fn ts_export_type_is_skipped() {
        let p = parse("export type Result = { ok: true } | { ok: false };").unwrap();
        assert_eq!(p.body.len(), 1);
        assert!(matches!(&p.body[0].kind, StmtKind::Empty));
    }

    #[test]
    fn ts_export_interface_is_skipped() {
        let p = parse("export interface Config { debug: boolean; port: number; }").unwrap();
        assert_eq!(p.body.len(), 1);
        assert!(matches!(&p.body[0].kind, StmtKind::Empty));
    }

    #[test]
    fn ts_mixed_types_and_runtime_code() {
        let p = parse(
            "type Num = number;\ninterface Obj { x: number; }\nlet a: Num = 10;\nlet b = 20;\n"
        ).unwrap();
        // type -> Empty, interface -> Empty, let a -> Var, let b -> Var
        assert_eq!(p.body.len(), 4);
        assert!(matches!(&p.body[0].kind, StmtKind::Empty));
        assert!(matches!(&p.body[1].kind, StmtKind::Empty));
        assert!(matches!(&p.body[2].kind, StmtKind::Var { .. }));
        assert!(matches!(&p.body[3].kind, StmtKind::Var { .. }));
    }

    #[test]
    fn ts_optional_param_question_mark_is_stripped() {
        // `?` before `:` in function params
        // For now this should at least not crash - optional params become regular params
        let p = parse("function f(x: number, y?: number): number { return x; }").unwrap();
        assert!(matches!(&p.body[0].kind, StmtKind::Function(f) if f.params.len() == 2));
    }

    #[test]
    fn dynamic_import_is_rejected() {
        let error = parse("import(\"./dep.js\").then(() => {});").unwrap_err();
        assert!(
            error.to_string().contains("dynamic import is not supported"),
            "{error}"
        );
    }

    #[test]
    fn import_meta_is_rejected() {
        let error = parse("import.meta.url;").unwrap_err();
        assert!(
            error.to_string().contains("import.meta is not supported"),
            "{error}"
        );
    }
}
