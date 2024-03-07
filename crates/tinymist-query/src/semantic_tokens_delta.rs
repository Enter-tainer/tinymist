use crate::{prelude::*, SemanticTokenCache};

#[derive(Debug, Clone)]
pub struct SemanticTokensDeltaRequest {
    pub path: PathBuf,
    pub previous_result_id: String,
}

pub fn semantic_tokens_delta(
    cache: &SemanticTokenCache,
    source: Source,
    req: SemanticTokensDeltaRequest,
    position_encoding: PositionEncoding,
) -> Option<SemanticTokensFullDeltaResult> {
    let (tokens, result_id) = cache.try_semantic_tokens_delta_from_result_id(
        &source,
        &req.previous_result_id,
        position_encoding,
    );

    match tokens {
        Ok(edits) => Some(
            SemanticTokensDelta {
                result_id: Some(result_id),
                edits,
            }
            .into(),
        ),
        Err(tokens) => Some(
            SemanticTokens {
                result_id: Some(result_id),
                data: tokens,
            }
            .into(),
        ),
    }
}
