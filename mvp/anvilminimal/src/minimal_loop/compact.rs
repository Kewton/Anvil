use crate::state::ConversationMessage;

pub fn compact_if_needed(messages: &mut Vec<ConversationMessage>, context_budget: usize) {
    let approx_chars = context_budget.saturating_mul(4);
    let mut total: usize = messages.iter().map(|m| m.content.len()).sum();
    while total > approx_chars && messages.len() > 4 {
        let removed = messages.remove(1);
        total = total.saturating_sub(removed.content.len());
    }
}
