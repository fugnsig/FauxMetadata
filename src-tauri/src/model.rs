use serde::Serialize;

pub type AppResult<T> = Result<T, String>;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub key: String,
    pub value: String,
    pub category: String,
    pub retained: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub id: String,
    pub name: String,
    pub path: String,
    pub format: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub preview: String,
    pub fields: Vec<Field>,
    pub chunks: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportIssue {
    pub path: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct ImportResult {
    pub images: Vec<Inspection>,
    pub issues: Vec<ImportIssue>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub id: String,
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub removed: Vec<Field>,
    pub retained: Vec<Field>,
    pub after: Vec<Field>,
    pub removed_chunks: Vec<String>,
    pub bytes: u64,
    pub encoded_verified: bool,
    pub pixels_verified: bool,
}

impl ExportResult {
    pub fn failed(id: &str, error: String, cancelled: bool) -> Self {
        Self {
            id: id.into(),
            status: if cancelled { "cancelled" } else { "error" }.into(),
            output: None,
            error: Some(error),
            removed: vec![],
            retained: vec![],
            after: vec![],
            removed_chunks: vec![],
            bytes: 0,
            encoded_verified: false,
            pixels_verified: false,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub job: String,
    pub id: String,
    pub phase: String,
    pub completed: usize,
    pub total: usize,
    pub result: Option<ExportResult>,
}
