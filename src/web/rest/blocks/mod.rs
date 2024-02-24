pub mod blocks;

use std::sync::Arc;

use actix_web::{
    get,
    http::header::ContentType,
    web::{self, Data},
    HttpResponse,
};

use crate::{
    block::{store::BlockStore, BlockHash},
    store::IndexerStore,
    web::rest::blocks::blocks::{Block, Blocks},
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Params {
    limit: Option<u32>,
}

#[get("/blocks")]
pub async fn get_blocks(
    store: Data<Arc<IndexerStore>>,
    params: web::Query<Params>,
) -> HttpResponse {
    let db = store.as_ref();
    let limit = params.limit.map(|value| value.min(10)).unwrap_or(1);

    if let Ok(Some(best_tip)) = db.get_best_block() {
        let mut parent_state_hash = best_tip.previous_state_hash();
        let rest_block: Block = Block::from(best_tip);
        let mut best_chain: Vec<Block> = vec![rest_block];
        let mut counter = 1;

        loop {
            if counter == limit {
                break;
            }
            if let Ok(Some(pcb)) = db.get_block(&parent_state_hash) {
                parent_state_hash = pcb.previous_state_hash();
                best_chain.push(Block::from(pcb));
            } else {
                // No parent
                break;
            }
            counter += 1;
        }
        let blocks = Blocks { blocks: best_chain };
        let body = serde_json::to_string_pretty(&blocks).unwrap();
        return HttpResponse::Ok()
            .content_type(ContentType::json())
            .body(body);
    }
    HttpResponse::NotFound().finish()
}

#[get("/blocks/{state_hash}")]
pub async fn get_block(
    store: Data<Arc<IndexerStore>>,
    state_hash: web::Path<String>,
) -> HttpResponse {
    let db = store.as_ref();
    if let Ok(Some(pcb)) = db.get_block(&BlockHash::from(state_hash.clone())) {
        let rest_block: Block = Block::from(pcb);
        let body = serde_json::to_string_pretty(&rest_block).unwrap();
        return HttpResponse::Ok()
            .content_type(ContentType::json())
            .body(body);
    }
    HttpResponse::NotFound().finish()
}
