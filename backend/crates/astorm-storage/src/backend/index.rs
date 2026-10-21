//! 全文索引后端：[`TantivyIndex`]（基于 Tantivy 倒排索引）。
//!
//! 面向消息的全文检索：`content` 分词建索引，`message_id` / `session_id` 作原始字符串存储，
//! 更新 = 删除（按 `message_id` term）+ 重插入，检索经 `QueryParser` 打分（TF-IDF）。

use std::path::Path;
use std::sync::Mutex;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, Schema, Value, INDEXED, STORED, STRING, TEXT};
use tantivy::{doc, Index, IndexWriter, TantivyDocument, Term};

use astorm_common::types::{MessageRecord, SearchHit, SearchIndex, StorageError};

/// 索引字段名常量。
const FIELD_MESSAGE_ID: &str = "message_id";
const FIELD_SESSION_ID: &str = "session_id";
const FIELD_ROLE: &str = "role";
const FIELD_CONTENT: &str = "content";
const FIELD_CREATED_AT: &str = "created_at";

/// Tantivy 全文索引后端。
pub struct TantivyIndex {
    index: Index,
    message_id: Field,
    session_id: Field,
    _role: Field,
    content: Field,
    _created_at: Field,
    /// 单写者（Tantivy 要求同一时刻只有一个 writer），互斥锁串行化。
    writer: Mutex<IndexWriter>,
}

impl TantivyIndex {
    /// 打开或创建索引目录：已存在则复用，否则按固定 schema 新建。
    pub fn open_or_create(dir: impl AsRef<Path>) -> Result<Self, StorageError> {
        let dir = dir.as_ref();
        // tantivy 的 open_in_dir / create_in_dir 均经 MmapDirectory::open，不自动建目录
        std::fs::create_dir_all(dir)
            .map_err(|e| StorageError::Index(format!("创建索引目录失败: {e}")))?;

        let mut schema_builder = Schema::builder();
        let message_id = schema_builder.add_text_field(FIELD_MESSAGE_ID, STRING | STORED);
        let session_id = schema_builder.add_text_field(FIELD_SESSION_ID, STRING | STORED);
        let role = schema_builder.add_text_field(FIELD_ROLE, STRING | STORED);
        let content = schema_builder.add_text_field(FIELD_CONTENT, TEXT | STORED);
        let created_at = schema_builder.add_i64_field(FIELD_CREATED_AT, INDEXED | STORED);
        let schema = schema_builder.build();

        let index = match Index::open_in_dir(dir) {
            Ok(index) => index,
            Err(_) => Index::create_in_dir(dir, schema.clone())
                .map_err(|e| StorageError::Index(e.to_string()))?,
        };

        let writer = index
            .writer(50_000_000)
            .map_err(|e| StorageError::Index(e.to_string()))?;

        Ok(Self {
            index,
            message_id,
            session_id,
            _role: role,
            content,
            _created_at: created_at,
            writer: Mutex::new(writer),
        })
    }

    /// 序列化文档写锁（中毒视为错误）。
    fn writer(&self) -> Result<std::sync::MutexGuard<'_, IndexWriter>, StorageError> {
        self.writer
            .lock()
            .map_err(|_| StorageError::Other("索引写锁中毒".into()))
    }
}

impl SearchIndex for TantivyIndex {
    fn index_messages(&self, messages: &[MessageRecord]) -> Result<(), StorageError> {
        let mut writer = self.writer()?;
        for msg in messages {
            writer.delete_term(Term::from_field_text(self.message_id, &msg.id));
            writer
                .add_document(doc!(
                    self.message_id => msg.id.clone(),
                    self.session_id => msg.session_id.clone(),
                    self.content => msg.content.clone(),
                ))
                .map_err(|e| StorageError::Index(e.to_string()))?;
        }
        writer
            .commit()
            .map_err(|e| StorageError::Index(e.to_string()))?;
        Ok(())
    }

    fn delete_messages(&self, ids: &[String]) -> Result<(), StorageError> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut writer = self.writer()?;
        for id in ids {
            writer.delete_term(Term::from_field_text(self.message_id, id));
        }
        writer
            .commit()
            .map_err(|e| StorageError::Index(e.to_string()))?;
        Ok(())
    }

    fn delete_session(&self, session_id: &str) -> Result<(), StorageError> {
        let mut writer = self.writer()?;
        writer.delete_term(Term::from_field_text(self.session_id, session_id));
        writer
            .commit()
            .map_err(|e| StorageError::Index(e.to_string()))?;
        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StorageError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| StorageError::Index(e.to_string()))?;
        let searcher = reader.searcher();

        let query_parser = QueryParser::for_index(&self.index, vec![self.content]);
        let query = query_parser
            .parse_query(query)
            .map_err(|e| StorageError::Index(format!("查询解析失败: {e}")))?;

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit).order_by_score())
            .map_err(|e| StorageError::Index(e.to_string()))?;

        let mut hits = Vec::with_capacity(top_docs.len());
        for (score, address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(address)
                .map_err(|e| StorageError::Index(e.to_string()))?;
            let session_id = doc
                .get_first(self.session_id)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let message_id = doc
                .get_first(self.message_id)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let snippet = doc
                .get_first(self.content)
                .and_then(|v| v.as_str())
                .map(|s| truncate(s, 200));
            hits.push(SearchHit {
                session_id,
                message_id,
                score,
                snippet,
            });
        }
        Ok(hits)
    }
}

/// 截取片段（UTF-8 安全，按字符截断）。
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect::<String>() + "…"
    }
}
