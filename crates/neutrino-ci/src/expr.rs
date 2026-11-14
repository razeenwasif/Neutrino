//! The `${{ }}` expression language, as much of it as Actions-shaped
//! workflows need: contexts (`neutrino`, `env`, `matrix`, `needs`, `vars`,
//! `secrets`), literals, `== != < <= > >=`, `&& || !`, parentheses, and the
//! functions `success() failure() always() cancelled() contains()
//! startsWith() endsWith() format() join() toJSON()`.
//!
//! Values are JSON values. As in Actions, `&&` and `||` return one of their
//! operands, string comparison ignores case, and `null`, `false`, `0` and `''`
//! are falsy.

use serde_json::{Map, Value};

use crate::Error;

/// How the jobs or steps before this one went, for the status functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    /// Something before this failed.
    pub failed: bool,
    /// The run was cancelled.
    pub cancelled: bool,
}

/// Everything an expression can read.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// `neutrino`, `env`, `matrix`, `needs`, `vars`, `secrets`, … as one object.
    pub values: Map<String, Value>,
    pub status: Status,
}

impl Context {
    pub fn with(mut self, name: &str, value: Value) -> Self {
        self.values.insert(name.to_string(), value);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Str(String),
    Num(f64),
    Ident(String),
    Op(&'static str),
}

fn tokens(src: &str) -> Result<Vec<Token>, Error> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => return Err(Error::new(format!("unterminated string in `{src}`"))),
                    Some('\'') if chars.get(i + 1) == Some(&'\'') => {
                        s.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(&ch) => {
                        s.push(ch);
                        i += 1;
                    }
                }
            }
            out.push(Token::Str(s));
            continue;
        }
        if c.is_ascii_digit() || (c == '-' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()) && !matches!(out.last(), Some(Token::Num(_) | Token::Str(_) | Token::Ident(_)) | Some(Token::Op(")" | "]")))) {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            out.push(Token::Num(text.parse().map_err(|_| Error::new(format!("bad number {text} in `{src}`")))?));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '-') {
                i += 1;
            }
            out.push(Token::Ident(chars[start..i].iter().collect()));
            continue;
        }
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        let op = ["==", "!=", "<=", ">=", "&&", "||"].into_iter().find(|o| *o == two);
        if let Some(op) = op {
            out.push(Token::Op(op));
            i += 2;
            continue;
        }
        let op = ["<", ">", "!", "(", ")", "[", "]", ".", ","].into_iter().find(|o| o.starts_with(c));
        match op {
            Some(op) => {
                out.push(Token::Op(op));
                i += 1;
            }
            None => return Err(Error::new(format!("unexpected `{c}` in `{src}`"))),
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Lit(Value),
    /// A context name and the keys or indexes after it.
    Path(String, Vec<Node>),
    Call(String, Vec<Node>),
    Not(Box<Node>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Cmp(&'static str, Box<Node>, Box<Node>),
}

struct Parser<'a> {
    toks: Vec<Token>,
    pos: usize,
    src: &'a str,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }
    fn eat(&mut self, op: &str) -> bool {
        if self.peek() == Some(&Token::Op(match op {
            "(" => "(",
            ")" => ")",
            "[" => "[",
            "]" => "]",
            "." => ".",
            "," => ",",
            "!" => "!",
            "&&" => "&&",
            "||" => "||",
            _ => return false,
        })) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn fail<T>(&self, what: &str) -> Result<T, Error> {
        Err(Error::new(format!("{what} in `{}`", self.src)))
    }
    fn or(&mut self) -> Result<Node, Error> {
        let mut left = self.and()?;
        while self.eat("||") {
            left = Node::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }
    fn and(&mut self) -> Result<Node, Error> {
        let mut left = self.not()?;
        while self.eat("&&") {
            left = Node::And(Box::new(left), Box::new(self.not()?));
        }
        Ok(left)
    }
    fn not(&mut self) -> Result<Node, Error> {
        if self.eat("!") {
            return Ok(Node::Not(Box::new(self.not()?)));
        }
        self.cmp()
    }
    fn cmp(&mut self) -> Result<Node, Error> {
        let left = self.primary()?;
        if let Some(Token::Op(op @ ("==" | "!=" | "<" | "<=" | ">" | ">="))) = self.peek().cloned() {
            self.pos += 1;
            let right = self.primary()?;
            return Ok(Node::Cmp(op, Box::new(left), Box::new(right)));
        }
        Ok(left)
    }
    fn primary(&mut self) -> Result<Node, Error> {
        match self.peek().cloned() {
            Some(Token::Str(s)) => {
                self.pos += 1;
                Ok(Node::Lit(Value::String(s)))
            }
            Some(Token::Num(n)) => {
                self.pos += 1;
                Ok(Node::Lit(serde_json::Number::from_f64(n).map(Value::Number).unwrap_or(Value::Null)))
            }
            Some(Token::Op("(")) => {
                self.pos += 1;
                let inner = self.or()?;
                if !self.eat(")") {
                    return self.fail("missing `)`");
                }
                Ok(inner)
            }
            Some(Token::Ident(name)) => {
                self.pos += 1;
                match name.as_str() {
                    "true" => return Ok(Node::Lit(Value::Bool(true))),
                    "false" => return Ok(Node::Lit(Value::Bool(false))),
                    "null" => return Ok(Node::Lit(Value::Null)),
                    _ => {}
                }
                if self.eat("(") {
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.or()?);
                            if self.eat(")") {
                                break;
                            }
                            if !self.eat(",") {
                                return self.fail("expected `,` or `)`");
                            }
                        }
                    }
                    return Ok(Node::Call(name.to_ascii_lowercase(), args));
                }
                let mut keys = Vec::new();
                loop {
                    if self.eat(".") {
                        match self.peek().cloned() {
                            Some(Token::Ident(k)) => {
                                self.pos += 1;
                                keys.push(Node::Lit(Value::String(k)));
                            }
                            _ => return self.fail("expected a name after `.`"),
                        }
                    } else if self.eat("[") {
                        keys.push(self.or()?);
                        if !self.eat("]") {
                            return self.fail("missing `]`");
                        }
                    } else {
                        break;
                    }
                }
                Ok(Node::Path(name, keys))
            }
            _ => self.fail("expected a value"),
        }
    }
}

/// A parsed expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr(Node);

impl Expr {
    pub fn parse(src: &str) -> Result<Expr, Error> {
        let mut p = Parser { toks: tokens(src)?, pos: 0, src };
        let node = p.or()?;
        if p.pos != p.toks.len() {
            return p.fail("unexpected text after the expression");
        }
        Ok(Expr(node))
    }

    /// Whether it calls `success()`, `failure()`, `always()` or `cancelled()`.
    pub fn checks_status(&self) -> bool {
        fn go(n: &Node) -> bool {
            match n {
                Node::Call(name, args) => matches!(name.as_str(), "success" | "failure" | "always" | "cancelled") || args.iter().any(go),
                Node::Not(a) => go(a),
                Node::And(a, b) | Node::Or(a, b) | Node::Cmp(_, a, b) => go(a) || go(b),
                Node::Path(_, keys) => keys.iter().any(go),
                Node::Lit(_) => false,
            }
        }
        go(&self.0)
    }

    pub fn eval(&self, cx: &Context) -> Result<Value, Error> {
        eval(&self.0, cx)
    }
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// How a value reads inside a string: `null` is empty, objects are JSON.
pub fn to_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn as_number(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) if s.trim().is_empty() => 0.0,
        Value::String(s) => s.trim().parse().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.eq_ignore_ascii_case(y),
        (Value::Null, Value::Null) => true,
        (Value::Array(_) | Value::Object(_), _) | (_, Value::Array(_) | Value::Object(_)) => a == b,
        _ => as_number(a) == as_number(b),
    }
}

fn eval(n: &Node, cx: &Context) -> Result<Value, Error> {
    Ok(match n {
        Node::Lit(v) => v.clone(),
        Node::Path(name, keys) => {
            let mut cur = cx.values.get(name.as_str()).cloned().unwrap_or(Value::Null);
            for k in keys {
                let key = eval(k, cx)?;
                cur = match (&cur, &key) {
                    (Value::Object(m), _) => {
                        let k = to_text(&key);
                        // Keys match without regard to case, as in Actions.
                        m.get(&k).or_else(|| m.iter().find(|(name, _)| name.eq_ignore_ascii_case(&k)).map(|(_, v)| v)).cloned().unwrap_or(Value::Null)
                    }
                    (Value::Array(a), Value::Number(i)) => i.as_u64().and_then(|i| a.get(i as usize)).cloned().unwrap_or(Value::Null),
                    _ => Value::Null,
                };
            }
            cur
        }
        Node::Not(a) => Value::Bool(!truthy(&eval(a, cx)?)),
        Node::And(a, b) => {
            let l = eval(a, cx)?;
            if truthy(&l) { eval(b, cx)? } else { l }
        }
        Node::Or(a, b) => {
            let l = eval(a, cx)?;
            if truthy(&l) { l } else { eval(b, cx)? }
        }
        Node::Cmp(op, a, b) => {
            let (l, r) = (eval(a, cx)?, eval(b, cx)?);
            Value::Bool(match *op {
                "==" => equal(&l, &r),
                "!=" => !equal(&l, &r),
                _ => {
                    let ord = match (&l, &r) {
                        (Value::String(x), Value::String(y)) => x.to_lowercase().partial_cmp(&y.to_lowercase()),
                        _ => as_number(&l).partial_cmp(&as_number(&r)),
                    };
                    match (*op, ord) {
                        (_, None) => false,
                        ("<", Some(o)) => o.is_lt(),
                        ("<=", Some(o)) => o.is_le(),
                        (">", Some(o)) => o.is_gt(),
                        (_, Some(o)) => o.is_ge(),
                    }
                }
            })
        }
        Node::Call(name, args) => {
            let vals = args.iter().map(|a| eval(a, cx)).collect::<Result<Vec<_>, _>>()?;
            let arity = |n: usize| -> Result<(), Error> {
                if vals.len() == n { Ok(()) } else { Err(Error::new(format!("{name}() takes {n} argument{}", if n == 1 { "" } else { "s" }))) }
            };
            match name.as_str() {
                "success" => {
                    arity(0)?;
                    Value::Bool(!cx.status.failed && !cx.status.cancelled)
                }
                "failure" => {
                    arity(0)?;
                    Value::Bool(cx.status.failed)
                }
                "always" => {
                    arity(0)?;
                    Value::Bool(true)
                }
                "cancelled" => {
                    arity(0)?;
                    Value::Bool(cx.status.cancelled)
                }
                "contains" => {
                    arity(2)?;
                    Value::Bool(match &vals[0] {
                        Value::Array(items) => items.iter().any(|i| equal(i, &vals[1])),
                        hay => to_text(hay).to_lowercase().contains(&to_text(&vals[1]).to_lowercase()),
                    })
                }
                "startswith" => {
                    arity(2)?;
                    Value::Bool(to_text(&vals[0]).to_lowercase().starts_with(&to_text(&vals[1]).to_lowercase()))
                }
                "endswith" => {
                    arity(2)?;
                    Value::Bool(to_text(&vals[0]).to_lowercase().ends_with(&to_text(&vals[1]).to_lowercase()))
                }
                "format" => {
                    let Some((fmt, rest)) = vals.split_first() else { return Err(Error::new("format() needs a format string")) };
                    let mut out = to_text(fmt);
                    for (i, v) in rest.iter().enumerate() {
                        out = out.replace(&format!("{{{i}}}"), &to_text(v));
                    }
                    Value::String(out)
                }
                "join" => {
                    let sep = vals.get(1).map(to_text).unwrap_or_else(|| ",".into());
                    match vals.first() {
                        Some(Value::Array(items)) => Value::String(items.iter().map(to_text).collect::<Vec<_>>().join(&sep)),
                        Some(v) => Value::String(to_text(v)),
                        None => return Err(Error::new("join() needs an array")),
                    }
                }
                "tojson" => {
                    arity(1)?;
                    Value::String(serde_json::to_string_pretty(&vals[0]).unwrap_or_default())
                }
                other => return Err(Error::new(format!("there's no function {other}()"))),
            }
        }
    })
}

/// Replace each `${{ … }}` in `text` with its value.
pub fn interpolate(text: &str, cx: &Context) -> Result<String, Error> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let Some(end) = after.find("}}") else { return Err(Error::new(format!("`${{{{` without `}}}}` in `{text}`"))) };
        let value = Expr::parse(after[..end].trim())?.eval(cx)?;
        out.push_str(&to_text(&value));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Check every `${{ … }}` in `text` parses, without evaluating.
pub fn check(text: &str) -> Result<(), Error> {
    let mut rest = text;
    while let Some(start) = rest.find("${{") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("}}") else { return Err(Error::new(format!("`${{{{` without `}}}}` in `{text}`"))) };
        Expr::parse(after[..end].trim())?;
        rest = &after[end + 2..];
    }
    Ok(())
}

/// An `if:` condition, which may be written bare or inside `${{ }}`. Without a
/// status function it only runs when everything before succeeded.
#[derive(Debug, Clone, PartialEq)]
pub struct Condition(Expr);

impl Condition {
    pub fn parse(src: &str) -> Result<Condition, Error> {
        let s = src.trim();
        let s = s.strip_prefix("${{").and_then(|x| x.strip_suffix("}}")).map(str::trim).unwrap_or(s);
        Ok(Condition(Expr::parse(s)?))
    }

    pub fn holds(&self, cx: &Context) -> Result<bool, Error> {
        let value = truthy(&self.0.eval(cx)?);
        Ok(if self.0.checks_status() { value } else { value && !cx.status.failed && !cx.status.cancelled })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cx() -> Context {
        Context::default()
            .with("neutrino", json!({"event": "push", "ref": "refs/heads/main", "branch": "main", "sha": "abc"}))
            .with("matrix", json!({"os": "linux", "n": 3}))
            .with("env", json!({"MODE": "fast"}))
            .with("needs", json!({"build": {"result": "success", "outputs": {}}}))
    }

    fn ev(s: &str) -> Value {
        Expr::parse(s).unwrap().eval(&cx()).unwrap()
    }

    #[test]
    fn values_and_operators() {
        assert_eq!(ev("neutrino.event == 'PUSH'"), json!(true), "strings compare without case");
        assert_eq!(ev("matrix.n > 2 && matrix.os"), json!("linux"), "&& returns its operand");
        assert_eq!(ev("matrix.missing || 'default'"), json!("default"));
        assert_eq!(ev("!(neutrino.branch != 'main')"), json!(true));
        assert_eq!(ev("needs.build.result"), json!("success"));
        assert_eq!(ev("matrix['os']"), json!("linux"));
        assert_eq!(ev("'it''s'"), json!("it's"));
        assert_eq!(ev("-1 < 0"), json!(true));
        assert_eq!(ev("contains(neutrino.ref, 'heads')"), json!(true));
        assert_eq!(ev("startsWith(neutrino.ref, 'refs/tags/')"), json!(false));
        assert_eq!(ev("format('{0}-{1}', matrix.os, matrix.n)"), json!("linux-3"));
        assert!(Expr::parse("neutrino.event ==").is_err());
        assert!(Expr::parse("nope(").is_err());
        assert!(Expr::parse("unknown()").unwrap().eval(&cx()).is_err());
    }

    #[test]
    fn interpolation() {
        assert_eq!(interpolate("cargo test --${{ env.MODE }} on ${{ matrix.os }}/${{ matrix.n }}", &cx()).unwrap(), "cargo test --fast on linux/3");
        assert_eq!(interpolate("${{ matrix.none }}x", &cx()).unwrap(), "x");
        assert!(interpolate("${{ broken", &cx()).is_err());
    }

    #[test]
    fn conditions() {
        let mut c = cx();
        assert!(Condition::parse("neutrino.branch == 'main'").unwrap().holds(&c).unwrap());
        assert!(Condition::parse("${{ neutrino.event == 'push' }}").unwrap().holds(&c).unwrap());
        c.status.failed = true;
        assert!(!Condition::parse("neutrino.branch == 'main'").unwrap().holds(&c).unwrap(), "a failure stops it unless it asks");
        assert!(Condition::parse("failure()").unwrap().holds(&c).unwrap());
        assert!(Condition::parse("always()").unwrap().holds(&c).unwrap());
        c.status = Status { failed: false, cancelled: true };
        assert!(!Condition::parse("success()").unwrap().holds(&c).unwrap());
        assert!(Condition::parse("cancelled()").unwrap().holds(&c).unwrap());
    }
}
