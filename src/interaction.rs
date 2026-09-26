use crate::{
    Client, Error, Response,
    generated::models,
    resources::{EntryResource, RunResource, ThreadResource},
};
use std::time::Duration;

/// Acceptance receipt with canonical references. A queued entry has no Run.
pub struct Submitted<'a> {
    pub receipt: Response<models::Submitted>,
    pub thread: ThreadResource<'a>,
    pub entry: EntryResource<'a>,
    pub run: Option<RunResource<'a>>,
}
impl<'a> Submitted<'a> {
    pub(crate) fn bind(
        client: &'a Client,
        receipt: Response<models::Submitted>,
    ) -> Result<Self, Error> {
        let value = &receipt.data;
        if value.thread.workspace_id.is_empty()
            || value.thread.id.is_empty()
            || value.entry.id.is_empty()
            || value.run.as_ref().is_some_and(|run| run.id.is_empty())
        {
            return Err(Error::Protocol);
        }
        let workspace = client
            .resources()
            .workspaces()
            .at(value.thread.workspace_id.clone());
        let thread = workspace.threads().at(value.thread.id.clone());
        let entry = thread.inbox().at(value.entry.id.clone());
        let run = value
            .run
            .as_ref()
            .map(|run| workspace.runs().at(run.id.clone()));
        Ok(Self {
            receipt,
            thread,
            entry,
            run,
        })
    }
}
/// Convenience over the full generated MessagePayload union.
pub fn text_payload(text: impl Into<String>) -> models::MessagePayload {
    models::MessagePayload::new(vec![models::Part::Text(Box::new(models::TextPart::new(
        text.into(),
        models::text_part::Type::Text,
    )))])
}
impl RunResource<'_> {
    /// Observe exactly this Run. Drop or wrap this entire future in tokio::time::timeout
    /// to cancel waiting; this does not stop the Run or follow its successor.
    pub async fn wait(&self, interval: Duration) -> Result<Response<models::RunView>, Error> {
        let interval = if interval.is_zero() {
            Duration::from_millis(250)
        } else {
            interval
        };
        self.0
            .client
            .observe(async {
                loop {
                    let result = self.get().await?;
                    if matches!(
                        result.data.status,
                        models::RunStatus::Completed
                            | models::RunStatus::Waiting
                            | models::RunStatus::Failed
                            | models::RunStatus::Cancelled
                    ) {
                        return Ok(result);
                    }
                    tokio::time::sleep(interval).await;
                }
            })
            .await
    }
}
impl EntryResource<'_> {
    /// Assignment alone is not consumption. Cancellation covers requests and sleeps.
    pub async fn wait(&self, interval: Duration) -> Result<Response<models::EntryView>, Error> {
        let interval = if interval.is_zero() {
            Duration::from_millis(250)
        } else {
            interval
        };
        self.0
            .client
            .observe(async {
                loop {
                    let result = self.get().await?;
                    if matches!(
                        result.data.status,
                        models::EntryStatus::Consumed
                            | models::EntryStatus::Failed
                            | models::EntryStatus::Withdrawn
                    ) {
                        return Ok(result);
                    }
                    tokio::time::sleep(interval).await;
                }
            })
            .await
    }
}
