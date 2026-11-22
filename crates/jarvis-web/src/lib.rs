//! Интернет для J.A.R.V.I.S. (план, разделы 3в и 3г): «гуглить как Claude».
//!
//! - [`search`] — trait [`SearchProvider`] и реализации (DuckDuckGo HTML без ключа, Brave, Tavily, Serper,
//!   Google Programmable Search, Jina), цепочка [`SearchChain`] с failover и [`validate`] для кнопки «Проверить».
//! - [`fetch`] — [`Fetcher`]: страница → очищенный Markdown (readability + HTML→Markdown), PDF → текст,
//!   лимит ~12k токенов с пометкой обрезки, запасной Jina Reader для JS-страниц.
//! - [`research`] — [`Researcher`]: LLM планирует запросы → поиск → чтение 3–5 страниц с разных доменов →
//!   синтез по-русски со ссылками [1], [2]. Каждая страница оборачивается в `<untrusted>`.
//! - [`tools`] — инструменты агента `web_search`, `web_fetch`, `web_research`, `web_open`.

pub mod extract;
pub mod fetch;
pub mod http;
pub mod research;
pub mod search;
pub mod tools;

mod text;

pub use fetch::{FetchError, FetchOptions, Fetcher, Page, PageFetcher, fetch};
pub use research::{ResearchError, ResearchOptions, ResearchResult, Researcher};
pub use search::{
    SearchChain, SearchError, SearchHit, SearchProvider, ValidateReport, provider_from_config, validate,
    validate_config,
};
pub use tools::{Opener, SystemOpener, WebState, WebStack, web_tools};
