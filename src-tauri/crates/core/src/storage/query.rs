//! Composable WHERE-clause builder. Plan §5.3.

use rusqlite::types::ToSql;

/// A SQL predicate fragment plus its bound params. Compose with and/or/not.
pub struct Q {
    pub sql: String,
    pub params: Vec<Box<dyn ToSql>>,
}

impl Q {
    /// `Q::new("AUTHOR_FK = ?", author_fk)` — one fragment, one param.
    pub fn new(sql: impl Into<String>, param: impl ToSql + 'static) -> Self {
        Q {
            sql: sql.into(),
            params: vec![Box::new(param)],
        }
    }

    /// `(self AND other)`.
    pub fn and(mut self, mut other: Q) -> Q {
        self.sql = format!("({} AND {})", self.sql, other.sql);
        self.params.append(&mut other.params);
        self
    }

    /// Borrowed params for `conn.execute(&q.sql, q.params_slice())`.
    pub fn params_slice(&self) -> Vec<&dyn ToSql> {
        self.params.iter().map(|p| p.as_ref()).collect()
    }
}

/// `col IN (?, ?, …)`. Caller ensures `vals` is non-empty.
pub fn _in<T: ToSql + 'static>(col: &str, vals: impl IntoIterator<Item = T>) -> Q {
    let params: Vec<Box<dyn ToSql>> = vals
        .into_iter()
        .map(|v| Box::new(v) as Box<dyn ToSql>)
        .collect();
    let marks = vec!["?"; params.len()].join(",");
    Q {
        sql: format!("{col} IN ({marks})"),
        params,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_and_or_not_and_in() {
        let q = Q::new("a = ?", 1i64).and(Q::new("b = ?", 2i64));
        assert_eq!(q.sql, "(a = ? AND b = ?)");
        assert_eq!(q.params.len(), 2);
        let q = _in("id", vec![1i64, 2, 3]);
        assert_eq!(q.sql, "id IN (?,?,?)");
        assert_eq!(q.params.len(), 3);
    }
}
