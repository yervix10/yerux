// A small JSON reader/writer for the language server (the compiler has no dependencies).
use std::fmt::Write;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn int(&self) -> Option<i64> {
        match self {
            Json::Num(n) => Some(*n as i64),
            _ => None,
        }
    }
    pub fn arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    /// `a.b.c` lookup through nested objects.
    pub fn path(&self, p: &str) -> Option<&Json> {
        p.split('.').try_fold(self, |v, k| v.get(k))
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else {
                    let _ = write!(out, "{}", n);
                }
            }
            Json::Str(s) => quote(s, out),
            Json::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Json::Obj(kv) => {
                out.push('{');
                for (i, (k, v)) in kv.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    quote(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn obj(kv: Vec<(&str, Json)>) -> Json {
    Json::Obj(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}
pub fn s(x: impl Into<String>) -> Json {
    Json::Str(x.into())
}
pub fn n(x: usize) -> Json {
    Json::Num(x as f64)
}

pub fn parse(text: &str) -> Result<Json, String> {
    let cs: Vec<char> = text.chars().collect();
    let mut p = P { cs: &cs, i: 0, depth: 0 };
    let v = p.value()?;
    p.ws();
    if p.i != cs.len() {
        return Err("trailing characters after the JSON value".into());
    }
    Ok(v)
}

struct P<'a> {
    cs: &'a [char],
    i: usize,
    depth: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.cs.len() && self.cs[self.i].is_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: char) -> Result<(), String> {
        self.ws();
        if self.cs.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at {}", c, self.i))
        }
    }
    fn lit(&mut self, w: &str, v: Json) -> Result<Json, String> {
        if self.cs[self.i..].iter().take(w.len()).collect::<String>() == w {
            self.i += w.len();
            Ok(v)
        } else {
            Err(format!("bad literal at {}", self.i))
        }
    }
    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        self.depth += 1;
        if self.depth > 200 {
            return Err("JSON nested too deeply".into());
        }
        let r = match self.cs.get(self.i) {
            None => Err("unexpected end of JSON".into()),
            Some('n') => self.lit("null", Json::Null),
            Some('t') => self.lit("true", Json::Bool(true)),
            Some('f') => self.lit("false", Json::Bool(false)),
            Some('"') => self.string().map(Json::Str),
            Some('[') => {
                self.i += 1;
                let mut a = vec![];
                self.ws();
                if self.cs.get(self.i) == Some(&']') {
                    self.i += 1;
                } else {
                    loop {
                        a.push(self.value()?);
                        self.ws();
                        match self.cs.get(self.i) {
                            Some(',') => self.i += 1,
                            Some(']') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(format!("expected ',' or ']' at {}", self.i)),
                        }
                    }
                }
                Ok(Json::Arr(a))
            }
            Some('{') => {
                self.i += 1;
                let mut kv = vec![];
                self.ws();
                if self.cs.get(self.i) == Some(&'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.ws();
                        let k = self.string()?;
                        self.eat(':')?;
                        kv.push((k, self.value()?));
                        self.ws();
                        match self.cs.get(self.i) {
                            Some(',') => self.i += 1,
                            Some('}') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(format!("expected ',' or '}}' at {}", self.i)),
                        }
                    }
                }
                Ok(Json::Obj(kv))
            }
            Some(_) => {
                let st = self.i;
                while self.i < self.cs.len() && "+-0123456789.eE".contains(self.cs[self.i]) {
                    self.i += 1;
                }
                self.cs[st..self.i]
                    .iter()
                    .collect::<String>()
                    .parse::<f64>()
                    .map(Json::Num)
                    .map_err(|_| format!("bad number at {}", st))
            }
        };
        self.depth -= 1;
        r
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let h: String = self.cs.get(self.i..self.i + 4).ok_or("bad \\u escape")?.iter().collect();
        self.i += 4;
        u32::from_str_radix(&h, 16).map_err(|_| "bad \\u escape".to_string())
    }
    fn string(&mut self) -> Result<String, String> {
        if self.cs.get(self.i) != Some(&'"') {
            return Err(format!("expected a string at {}", self.i));
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let c = *self.cs.get(self.i).ok_or("unterminated string")?;
            self.i += 1;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let e = *self.cs.get(self.i).ok_or("unterminated escape")?;
                    self.i += 1;
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'u' => {
                            let mut u = self.hex4()?;
                            if (0xD800..0xDC00).contains(&u) && self.cs.get(self.i) == Some(&'\\') {
                                self.i += 2;
                                let lo = self.hex4()?;
                                u = 0x10000 + ((u - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3ff);
                            }
                            out.push(char::from_u32(u).unwrap_or('\u{fffd}'));
                        }
                        o => out.push(o),
                    }
                }
                c => out.push(c),
            }
        }
    }
}
