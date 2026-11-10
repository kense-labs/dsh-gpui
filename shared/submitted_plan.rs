use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct PlanDocument {
    pub(crate) title: String,
    pub(crate) markdown: String,
}

impl PlanDocument {
    pub(crate) fn from_logged_markdown(markdown: String) -> Option<Self> {
        let heading = markdown.trim().strip_prefix('#')?;
        if !heading.starts_with(char::is_whitespace) {
            return None;
        }
        let title = heading.trim_start().lines().next()?.to_owned();
        Some(Self { title, markdown })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubmittedPlan {
    pub(crate) call_id: String,
    #[serde(flatten)]
    pub(crate) document: PlanDocument,
}

impl SubmittedPlan {
    pub(crate) fn from_event(event: &Value) -> Option<Self> {
        let data = &event["data"];
        if data["name"] != "exit_plan_mode" {
            return None;
        }
        let (id, markdown) = match event["type"].as_str()? {
            "tool/call" => {
                let args: Value = serde_json::from_str(data["arguments"].as_str()?).ok()?;
                (data["callId"].as_str()?, args["plan"].as_str()?.to_owned())
            }
            "tool/ptc-dispatch-start" | "tool/ptc-dispatch" => (
                data["subCallId"].as_str()?,
                data["arguments"]["plan"].as_str()?.to_owned(),
            ),
            _ => return None,
        };
        if id.is_empty() {
            return None;
        }
        Some(Self {
            call_id: id.to_owned(),
            document: PlanDocument::from_logged_markdown(markdown)?,
        })
    }
}
