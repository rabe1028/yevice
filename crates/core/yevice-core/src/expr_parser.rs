//! Simple expression parser for binding formulas.
//!
//! Supports:
//! - Variable references: `FunctionName_requests`
//! - Numeric literals: `42`, `0.7`, `1000`
//! - Arithmetic: `+`, `-`, `*`, `/`
//! - Parentheses: `(a + b) * c`
//! - `ceil(expr)`: ceiling function
//! - `max(expr, floor)`: maximum of expr and a constant floor value
//! - `min(expr, ceiling)`: minimum of expr and a constant ceiling value
//!
//! Grammar (recursive descent):
//!   expr     = term (('+' | '-') term)*
//!   term     = unary (('*' | '/') unary)*
//!   unary    = 'ceil' '(' expr ')'
//!            | 'max' '(' expr ',' NUMBER ')'
//!            | 'min' '(' expr ',' NUMBER ')'
//!            | atom
//!   atom     = NUMBER | VARIABLE | '(' expr ')'

use crate::expr::Expr;
use crate::types::VariableName;

#[derive(Debug)]
pub enum ParseError {
    UnexpectedChar(char),
    UnexpectedEnd,
    ExpectedCloseParen,
    ExpectedComma,
    ExpectedNumber,
    InvalidNumber { literal: String, pos: usize },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedChar(c) => write!(f, "unexpected character: '{c}'"),
            Self::UnexpectedEnd => write!(f, "unexpected end of expression"),
            Self::ExpectedCloseParen => write!(f, "expected ')'"),
            Self::ExpectedComma => write!(f, "expected ','"),
            Self::ExpectedNumber => write!(f, "expected a numeric literal"),
            Self::InvalidNumber { literal, pos } => {
                write!(f, "invalid number literal '{literal}' at position {pos}")
            }
        }
    }
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_whitespace();
        self.input.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        self.skip_whitespace();
        if self.pos < self.input.len() {
            let c = self.input[self.pos];
            self.pos += 1;
            Some(c)
        } else {
            None
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_term()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.advance();
                    let right = self.parse_term()?;
                    left = Expr::sum(vec![left, right]);
                }
                Some(b'-') => {
                    self.advance();
                    let right = self.parse_term()?;
                    left = Expr::sum(vec![left, Expr::product(vec![Expr::constant(-1.0), right])]);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_term(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.advance();
                    let right = self.parse_unary()?;
                    left = Expr::product(vec![left, right]);
                }
                Some(b'/') => {
                    self.advance();
                    let right = self.parse_unary()?;
                    left = Expr::div(left, right);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        self.skip_whitespace();

        if self.starts_with_ident("ceil") {
            self.pos += 4;
            self.skip_whitespace();
            if self.advance() != Some(b'(') {
                return Err(ParseError::ExpectedCloseParen);
            }
            let inner = self.parse_expr()?;
            self.skip_whitespace();
            if self.advance() != Some(b')') {
                return Err(ParseError::ExpectedCloseParen);
            }
            return Ok(Expr::ceil(inner));
        }

        if self.starts_with_ident("max") {
            self.pos += 3;
            self.skip_whitespace();
            if self.advance() != Some(b'(') {
                return Err(ParseError::ExpectedCloseParen);
            }
            let inner = self.parse_expr()?;
            self.skip_whitespace();
            if self.advance() != Some(b',') {
                return Err(ParseError::ExpectedComma);
            }
            self.skip_whitespace();
            let floor = self.parse_number_literal()?;
            self.skip_whitespace();
            if self.advance() != Some(b')') {
                return Err(ParseError::ExpectedCloseParen);
            }
            return Ok(Expr::Max {
                expr: Box::new(inner),
                floor,
            });
        }

        if self.starts_with_ident("min") {
            self.pos += 3;
            self.skip_whitespace();
            if self.advance() != Some(b'(') {
                return Err(ParseError::ExpectedCloseParen);
            }
            let inner = self.parse_expr()?;
            self.skip_whitespace();
            if self.advance() != Some(b',') {
                return Err(ParseError::ExpectedComma);
            }
            self.skip_whitespace();
            let ceiling = self.parse_number_literal()?;
            self.skip_whitespace();
            if self.advance() != Some(b')') {
                return Err(ParseError::ExpectedCloseParen);
            }
            return Ok(Expr::Min {
                expr: Box::new(inner),
                ceiling,
            });
        }

        self.parse_atom()
    }

    fn parse_number_literal(&mut self) -> Result<f64, ParseError> {
        self.skip_whitespace();
        match self.peek() {
            Some(c) if c.is_ascii_digit() || c == b'.' => {}
            _ => return Err(ParseError::ExpectedNumber),
        }
        let start = self.pos;
        while self.pos < self.input.len()
            && (self.input[self.pos].is_ascii_digit() || self.input[self.pos] == b'.')
        {
            self.pos += 1;
        }
        let s = std::str::from_utf8(&self.input[start..self.pos]).unwrap();
        s.parse::<f64>().map_err(|_| ParseError::InvalidNumber {
            literal: s.to_string(),
            pos: start,
        })
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'(') => {
                self.advance();
                let expr = self.parse_expr()?;
                self.skip_whitespace();
                if self.advance() != Some(b')') {
                    return Err(ParseError::ExpectedCloseParen);
                }
                Ok(expr)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => self.parse_variable(),
            Some(c) => Err(ParseError::UnexpectedChar(c as char)),
            None => Err(ParseError::UnexpectedEnd),
        }
    }

    fn parse_number(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        while self.pos < self.input.len()
            && (self.input[self.pos].is_ascii_digit() || self.input[self.pos] == b'.')
        {
            self.pos += 1;
        }
        let s = std::str::from_utf8(&self.input[start..self.pos]).unwrap();
        let value: f64 = s.parse().map_err(|_| ParseError::InvalidNumber {
            literal: s.to_string(),
            pos: start,
        })?;
        Ok(Expr::constant(value))
    }

    fn parse_variable(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        while self.pos < self.input.len()
            && (self.input[self.pos].is_ascii_alphanumeric() || self.input[self.pos] == b'_')
        {
            self.pos += 1;
        }
        let name = std::str::from_utf8(&self.input[start..self.pos]).unwrap();
        Ok(Expr::variable(VariableName::new(name)))
    }

    fn starts_with_ident(&self, s: &str) -> bool {
        if self.pos + s.len() > self.input.len() {
            return false;
        }
        if &self.input[self.pos..self.pos + s.len()] != s.as_bytes() {
            return false;
        }
        // Ensure next char is not alphanumeric
        if self.pos + s.len() < self.input.len()
            && (self.input[self.pos + s.len()].is_ascii_alphanumeric()
                || self.input[self.pos + s.len()] == b'_')
        {
            return false;
        }
        true
    }
}

/// Parse an expression string into an Expr AST.
pub fn parse_expr(input: &str) -> Result<Expr, ParseError> {
    let mut parser = Parser::new(input);
    let expr = parser.parse_expr()?;
    parser.skip_whitespace();
    if parser.pos < parser.input.len() {
        return Err(ParseError::UnexpectedChar(parser.input[parser.pos] as char));
    }
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluate::Params;

    fn params_from(pairs: &[(&str, f64)]) -> Params {
        pairs
            .iter()
            .map(|(k, v)| (VariableName::new(*k), *v))
            .collect()
    }

    fn compute(input: &str, params: &Params) -> f64 {
        let expr = parse_expr(input).unwrap();
        crate::evaluate::evaluate(&expr, params).unwrap()
    }

    #[test]
    fn test_constant() {
        assert_eq!(compute("42", &Params::default()), 42.0);
        assert_eq!(compute("0.7", &Params::default()), 0.7);
    }

    #[test]
    fn test_variable() {
        let p = params_from(&[("x", 10.0)]);
        assert_eq!(compute("x", &p), 10.0);
    }

    #[test]
    fn test_arithmetic() {
        let p = params_from(&[("a", 100.0), ("b", 3.0)]);
        assert_eq!(compute("a * b", &p), 300.0);
        assert!((compute("a / b", &p) - 100.0 / 3.0).abs() < 1e-10);
        assert_eq!(compute("a + b", &p), 103.0);
        assert_eq!(compute("a - b", &p), 97.0);
    }

    #[test]
    fn test_precedence() {
        let p = params_from(&[("a", 2.0), ("b", 3.0), ("c", 4.0)]);
        assert_eq!(compute("a + b * c", &p), 14.0);
        assert_eq!(compute("(a + b) * c", &p), 20.0);
    }

    #[test]
    fn test_ceil() {
        let p = params_from(&[("a", 10.0), ("b", 3.0)]);
        assert_eq!(compute("ceil(a / b)", &p), 4.0);
    }

    #[test]
    fn test_s3_storage_formula() {
        let p = params_from(&[
            ("executions", 1000.0),
            ("avg_object_size_gb", 0.7),
            ("retention_days", 7.0),
        ]);
        let result = compute("executions * avg_object_size_gb * retention_days / 30", &p);
        assert!((result - 163.333).abs() < 0.01);
    }

    #[test]
    fn test_batch_invocations() {
        let p = params_from(&[("transitions", 3000.0)]);
        assert_eq!(compute("ceil(transitions / 3)", &p), 1000.0);
    }

    #[test]
    fn test_malformed_number_error_contains_literal() {
        let err = parse_expr("1.2.3").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("1.2.3"),
            "error message must contain the offending literal; got: {msg}"
        );
    }

    #[test]
    fn test_max_below_floor() {
        // x = 2, floor = 5  →  max(2, 5) = 5
        let p = params_from(&[("x", 2.0)]);
        assert_eq!(compute("max(x, 5)", &p), 5.0);
    }

    #[test]
    fn test_max_above_floor() {
        // x = 10, floor = 5  →  max(10, 5) = 10
        let p = params_from(&[("x", 10.0)]);
        assert_eq!(compute("max(x, 5)", &p), 10.0);
    }

    #[test]
    fn test_min_below_ceiling() {
        // x = 2, ceiling = 5  →  min(2, 5) = 2
        let p = params_from(&[("x", 2.0)]);
        assert_eq!(compute("min(x, 5)", &p), 2.0);
    }

    #[test]
    fn test_min_above_ceiling() {
        // x = 10, ceiling = 5  →  min(10, 5) = 5
        let p = params_from(&[("x", 10.0)]);
        assert_eq!(compute("min(x, 5)", &p), 5.0);
    }

    #[test]
    fn test_max_nested_with_ceil_and_div() {
        // ceil(max(x, 1) / 3) with x=7  →  ceil(7/3) = ceil(2.333..) = 3
        let p = params_from(&[("x", 7.0)]);
        assert_eq!(compute("ceil(max(x, 1) / 3)", &p), 3.0);
        // x=0  →  ceil(max(0, 1) / 3) = ceil(1/3) = ceil(0.333..) = 1
        let p0 = params_from(&[("x", 0.0)]);
        assert_eq!(compute("ceil(max(x, 1) / 3)", &p0), 1.0);
    }

    #[test]
    fn test_max_missing_comma_error() {
        let err = parse_expr("max(x 5)").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(','),
            "error message should mention comma; got: {msg}"
        );
    }

    #[test]
    fn test_max_non_literal_second_arg_error() {
        // The second argument of max/min must be a numeric literal.
        let err = parse_expr("max(x, y)").unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.is_empty(),
            "expected an error for non-literal second arg; got: {msg}"
        );
    }

    #[test]
    fn test_min_nested() {
        // min(x * 2, 10) with x=3  →  min(6, 10) = 6
        let p = params_from(&[("x", 3.0)]);
        assert_eq!(compute("min(x * 2, 10)", &p), 6.0);
        // x=8  →  min(16, 10) = 10
        let p8 = params_from(&[("x", 8.0)]);
        assert_eq!(compute("min(x * 2, 10)", &p8), 10.0);
    }
}
