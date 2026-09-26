//! Complete generated resource references and explicit lazy page iteration.
pub use crate::generated::resources::*;
use crate::{Client, Error, Response};
use reqwest::RequestBuilder;
use serde::Serialize;
use serde_json::Value;
use std::{collections::HashSet, future::Future};

#[derive(Clone)]
pub(crate) struct Binding<'a> {
    pub client: &'a Client,
    pub ids: Vec<String>,
}
impl<'a> Binding<'a> {
    pub fn select(&self, id: String) -> Self {
        let mut next = self.clone();
        next.ids.push(id);
        next
    }
}
pub(crate) fn query<T: Serialize>(
    mut request: RequestBuilder,
    name: &str,
    value: &T,
) -> Result<RequestBuilder, Error> {
    let value = serde_json::to_value(value).map_err(|_| Error::InvalidInput)?;
    let values = match value {
        Value::Array(values) => values,
        other => vec![other],
    };
    for value in values {
        let value = match value {
            Value::String(value) => value,
            other => other.to_string(),
        };
        request = request.query(&[(name, value)]);
    }
    Ok(request)
}

#[doc(hidden)]
pub trait PageSource {
    type Page;
    fn fetch(
        &self,
        cursor: Option<String>,
    ) -> impl Future<Output = Result<Response<Self::Page>, Error>> + Send;
    fn cursor(page: &Self::Page) -> Option<String>;
}
/// No prefetching. Options are owned snapshots; dropping the pager stops reads.
pub struct Pages<S> {
    source: S,
    cursor: Option<String>,
    seen: HashSet<String>,
    done: bool,
}
impl<S: PageSource> Pages<S> {
    pub(crate) fn new(source: S, cursor: Option<String>) -> Self {
        let mut seen = HashSet::new();
        if let Some(cursor) = &cursor {
            seen.insert(cursor.clone());
        }
        Self {
            source,
            cursor,
            seen,
            done: false,
        }
    }
    pub async fn next(&mut self) -> Result<Option<Response<S::Page>>, Error> {
        if self.done {
            return Ok(None);
        }
        let page = match self.source.fetch(self.cursor.clone()).await {
            Ok(page) => page,
            Err(error) => {
                self.done = true;
                return Err(error);
            }
        };
        self.cursor = S::cursor(&page.data).filter(|cursor| !cursor.is_empty());
        if let Some(cursor) = &self.cursor {
            if !self.seen.insert(cursor.clone()) {
                self.done = true;
                return Err(Error::Protocol);
            }
        } else {
            self.done = true;
        }
        Ok(Some(page))
    }
}
