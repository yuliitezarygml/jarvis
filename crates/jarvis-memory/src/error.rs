#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("ошибка базы памяти: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("ошибка JSON в памяти: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ошибка файловой системы: {0}")]
    Io(#[from] std::io::Error),
    #[error("в памяти нет записи {0} #{1}")]
    NotFound(&'static str, i64),
    #[error("некорректные данные: {0}")]
    Invalid(String),
    #[error("фоновая задача памяти завершилась с ошибкой: {0}")]
    Join(String),
}

pub type Result<T, E = MemoryError> = std::result::Result<T, E>;
