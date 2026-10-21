//! 知识库（kb_*）接口：真实文件系统 CRUD。

mod create;
mod delete;
mod dto;
mod import;
mod list;
mod read;
mod read_buffer;
mod rename;
mod root;
mod update_meta;
mod write;

pub use create::kb_create;
pub use delete::kb_delete;
pub use dto::{KbCreateRequest, KbItem, KbMetaUpdate};
pub use import::kb_import;
pub use list::kb_list;
pub use read::kb_read;
pub use read_buffer::kb_read_buffer;
pub use rename::kb_rename;
pub use root::get_knowledge_root;
pub use update_meta::kb_update_meta;
pub use write::kb_write;
