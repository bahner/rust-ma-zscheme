#![allow(dead_code)]
/// Scheme values and lexically-scoped environments for zscheme.
/// Ported from ma-agent/src/scheme/value.rs.
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fmt,
    rc::Rc,
};

use crate::parser::SchemeExpr;

// ── Environment ────────────────────────────────────────────────────────────

struct EnvInner {
    vars: HashMap<String, SchemeVal>,
    parent: Option<Env>,
}

/// A lexically-scoped environment frame.
/// `Rc<RefCell<…>>` gives cheap clone + interior mutability.
#[derive(Clone)]
pub struct Env(Rc<RefCell<EnvInner>>);

impl fmt::Debug for Env {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#<env>")
    }
}

impl Env {
    #[must_use]
    pub fn new_root() -> Self {
        Env(Rc::new(RefCell::new(EnvInner {
            vars: HashMap::new(),
            parent: None,
        })))
    }

    #[must_use]
    pub fn extend(parent: &Env) -> Self {
        Env(Rc::new(RefCell::new(EnvInner {
            vars: HashMap::new(),
            parent: Some(parent.clone()),
        })))
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<SchemeVal> {
        let inner = self.0.borrow();
        if let Some(v) = inner.vars.get(name) {
            return Some(v.clone());
        }
        inner.parent.as_ref().and_then(|p| p.get(name))
    }

    pub fn define(&self, name: impl Into<String>, val: SchemeVal) {
        self.0.borrow_mut().vars.insert(name.into(), val);
    }

    #[must_use]
    pub fn own_bindings(&self) -> Vec<(String, SchemeVal)> {
        self.0
            .borrow()
            .vars
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    #[must_use]
    pub fn set_existing(&self, name: &str, val: SchemeVal) -> Option<()> {
        let has_locally = self.0.borrow().vars.contains_key(name);
        if has_locally {
            self.0.borrow_mut().vars.insert(name.to_string(), val);
            return Some(());
        }
        let parent = self.0.borrow().parent.clone();
        parent?.set_existing(name, val)
    }
}

// ── Value ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum SchemeVal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Nil,
    List(Vec<SchemeVal>),
    Map(BTreeMap<String, SchemeVal>),
    /// A ma local config path reference (surface syntax `.my…`, `.ctx…`
    /// inside zscheme expressions): `.my.aliases.sky`, `.my.doc.poem!publish`,
    /// etc. Internally stored without the `#`.
    MaPath(String),
    /// A ma actor target: `@ma#house:enter`, `did:ma:abc#room:enter`, etc.
    MaActor(String),
    Builtin(String),
    Lambda {
        params: Vec<String>,
        rest: Option<String>,
        body: Vec<SchemeExpr>,
        env: Env,
    },
}

impl SchemeVal {
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            SchemeVal::Str(s) => s.clone(),
            SchemeVal::Int(n) => n.to_string(),
            SchemeVal::Float(f) => f.to_string(),
            SchemeVal::Bool(true) => "#t".to_string(),
            SchemeVal::Bool(false) => "#f".to_string(),
            SchemeVal::Nil => "()".to_string(),
            SchemeVal::List(v) => {
                let inner: Vec<_> = v.iter().map(SchemeVal::repr).collect();
                format!("({})", inner.join(" "))
            }
            SchemeVal::Map(m) => {
                let inner: Vec<_> = m
                    .iter()
                    .map(|(k, v)| format!("({k:?} . {})", v.repr()))
                    .collect();
                format!("#<map ({})>", inner.join(" "))
            }
            SchemeVal::MaPath(p) => p.clone(),
            SchemeVal::MaActor(a) => a.clone(),
            SchemeVal::Builtin(n) => format!("#<procedure:{n}>"),
            SchemeVal::Lambda { .. } => "#<lambda>".to_string(),
        }
    }

    #[must_use]
    pub fn repr(&self) -> String {
        match self {
            SchemeVal::Str(s) => format!("{s:?}"),
            other => other.display(),
        }
    }

    #[must_use]
    pub fn to_splice_lossy(&self) -> String {
        match self {
            SchemeVal::Str(s) => s.clone(),
            SchemeVal::Nil => String::new(),
            SchemeVal::List(v) => v
                .iter()
                .map(SchemeVal::to_splice_lossy)
                .collect::<Vec<_>>()
                .join(" "),
            SchemeVal::Map(_) => self.display(),
            other => other.display(),
        }
    }

    /// # Errors
    ///
    /// Returns `Err` if the value is a lambda or builtin that cannot be
    /// represented as a plain string in a command context.
    pub fn to_splice(&self) -> Result<String, String> {
        match self {
            SchemeVal::Lambda { .. } => {
                Err("cannot splice a lambda into a command string".to_string())
            }
            SchemeVal::Builtin(n) => {
                Err(format!("cannot splice builtin '{n}' into a command string"))
            }
            other => Ok(other.to_splice_lossy()),
        }
    }

    #[must_use]
    pub fn is_truthy(&self) -> bool {
        !matches!(self, SchemeVal::Bool(false) | SchemeVal::Nil)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Env ──

    #[test]
    fn env_define_and_get() {
        let env = Env::new_root();
        env.define("x", SchemeVal::Int(42));
        assert!(matches!(env.get("x"), Some(SchemeVal::Int(42))));
    }

    #[test]
    fn env_get_missing_returns_none() {
        let env = Env::new_root();
        assert!(env.get("missing").is_none());
    }

    #[test]
    fn env_extend_inherits_parent() {
        let parent = Env::new_root();
        parent.define("x", SchemeVal::Int(1));
        let child = Env::extend(&parent);
        assert!(matches!(child.get("x"), Some(SchemeVal::Int(1))));
    }

    #[test]
    fn env_child_shadows_parent() {
        let parent = Env::new_root();
        parent.define("x", SchemeVal::Int(1));
        let child = Env::extend(&parent);
        child.define("x", SchemeVal::Int(2));
        assert!(matches!(child.get("x"), Some(SchemeVal::Int(2))));
        assert!(matches!(parent.get("x"), Some(SchemeVal::Int(1))));
    }

    #[test]
    fn env_set_existing_updates_value() {
        let env = Env::new_root();
        env.define("x", SchemeVal::Int(1));
        assert!(env.set_existing("x", SchemeVal::Int(99)).is_some());
        assert!(matches!(env.get("x"), Some(SchemeVal::Int(99))));
    }

    #[test]
    fn env_set_existing_missing_returns_none() {
        let env = Env::new_root();
        assert!(env.set_existing("nope", SchemeVal::Int(1)).is_none());
    }

    #[test]
    fn env_set_existing_updates_parent() {
        let parent = Env::new_root();
        parent.define("x", SchemeVal::Int(1));
        let child = Env::extend(&parent);
        let _ = child.set_existing("x", SchemeVal::Int(7));
        assert!(matches!(parent.get("x"), Some(SchemeVal::Int(7))));
    }

    // ── SchemeVal::display ──

    #[test]
    fn display_int() {
        assert_eq!(SchemeVal::Int(42).display(), "42");
    }

    #[test]
    fn display_bool() {
        assert_eq!(SchemeVal::Bool(true).display(), "#t");
        assert_eq!(SchemeVal::Bool(false).display(), "#f");
    }

    #[test]
    fn display_nil() {
        assert_eq!(SchemeVal::Nil.display(), "()");
    }

    #[test]
    fn display_str() {
        assert_eq!(SchemeVal::Str("hello".into()).display(), "hello");
    }

    #[test]
    fn display_list() {
        let v = SchemeVal::List(vec![SchemeVal::Int(1), SchemeVal::Int(2)]);
        assert_eq!(v.display(), "(1 2)");
    }

    #[test]
    fn display_map() {
        let mut map = BTreeMap::new();
        map.insert(
            "north".to_string(),
            SchemeVal::Str("did:ma:room#exit".to_string()),
        );
        let v = SchemeVal::Map(map);
        assert_eq!(v.display(), "#<map ((\"north\" . \"did:ma:room#exit\"))>");
    }

    // ── SchemeVal::repr ──

    #[test]
    fn repr_str_is_quoted() {
        assert_eq!(SchemeVal::Str("hi".into()).repr(), "\"hi\"");
    }

    #[test]
    fn repr_int_same_as_display() {
        assert_eq!(SchemeVal::Int(7).repr(), "7");
    }

    // ── SchemeVal::is_truthy ──

    #[test]
    fn only_false_and_nil_are_falsy() {
        assert!(!SchemeVal::Bool(false).is_truthy());
        assert!(!SchemeVal::Nil.is_truthy());
        assert!(SchemeVal::Bool(true).is_truthy());
        assert!(SchemeVal::Int(0).is_truthy());
        assert!(SchemeVal::Str(String::new()).is_truthy());
    }

    // ── SchemeVal::to_splice_lossy ──

    #[test]
    fn to_splice_lossy_str() {
        assert_eq!(SchemeVal::Str("world".into()).to_splice_lossy(), "world");
    }

    #[test]
    fn to_splice_lossy_nil_is_empty() {
        assert_eq!(SchemeVal::Nil.to_splice_lossy(), "");
    }

    #[test]
    fn to_splice_lossy_list_space_joined() {
        let v = SchemeVal::List(vec![SchemeVal::Str("a".into()), SchemeVal::Str("b".into())]);
        assert_eq!(v.to_splice_lossy(), "a b");
    }

    #[test]
    fn to_splice_lossy_map_displays_map() {
        let mut map = BTreeMap::new();
        map.insert("a".to_string(), SchemeVal::Int(1));
        assert_eq!(
            SchemeVal::Map(map).to_splice_lossy(),
            "#<map ((\"a\" . 1))>"
        );
    }

    // ── SchemeVal::to_splice ──

    #[test]
    fn to_splice_lambda_is_err() {
        let v = SchemeVal::Lambda {
            params: vec![],
            rest: None,
            body: vec![],
            env: Env::new_root(),
        };
        assert!(v.to_splice().is_err());
    }

    #[test]
    fn to_splice_str_is_ok() {
        assert_eq!(
            SchemeVal::Str("val".into()).to_splice(),
            Ok("val".to_string())
        );
    }
}
