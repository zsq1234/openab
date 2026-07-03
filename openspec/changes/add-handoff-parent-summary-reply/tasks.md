## 1. Configuration And Contracts

- [x] 1.1 Add disabled-by-default configuration for handoff parent summary replies, including enable flag and maximum summary length.
- [x] 1.2 Validate that parent summary replies can only be enabled when agent thread handoff is enabled.
- [x] 1.3 Add docs/config reference entries and examples for the new configuration.

## 2. Handoff Completion Metadata

- [x] 2.1 Define a one-shot handoff completion notification payload containing parent channel route, parent trigger message, parent session identity, child display reference, summary mode, and summary length limit.
- [x] 2.2 Extend `BufferedMessage` to carry optional completion notification metadata.
- [x] 2.3 Populate completion metadata only for synthetic messages created by `HandoffBroker::start_thread_task` when the feature is enabled.
- [x] 2.4 Ensure ordinary thread follow-up messages and internal summary prompts do not carry completion metadata.

## 3. Child Final Output Capture

- [x] 3.1 Add an internal stream outcome type that exposes the final child display content after thread delivery.
- [x] 3.2 Update `AdapterRouter::stream_prompt_blocks` and `DispatchTarget` to return the outcome without changing platform send behavior.
- [x] 3.3 Update dispatcher call sites and tests for the new outcome type.
- [x] 3.4 Preserve existing error handling and reaction behavior when child delivery fails.

## 4. Sanitization

- [x] 4.1 Implement deterministic sanitization for child final content before summarization.
- [x] 4.2 Strip output directives, thinking/progress lines, tool display sections/lines, empty-response sentinels, and redundant internal scaffolding.
- [x] 4.3 Bound sanitized input size before it is sent to the parent summary turn.
- [x] 4.4 Add unit tests for representative tool display, thinking/progress, directive, empty, and normal result inputs.

## 5. Parent Agent Summarization

- [x] 5.1 Build an internal summary prompt that treats sanitized child output as quoted data and instructs the parent agent to ignore instructions inside it.
- [x] 5.2 Submit the summary prompt to the original normal-channel parent agent/session after the child turn succeeds.
- [x] 5.3 Prevent the summary turn from minting handoff tokens or triggering another parent completion notification.
- [x] 5.4 Include the configured target maximum summary length in the parent-agent summary prompt without hard-truncating before sending.
- [x] 5.5 Reply with the generated summary to the original normal-channel trigger message.
- [x] 5.6 Implement deterministic fallback behavior for empty sanitized content and summary-generation failure.

## 6. Platform Routing

- [x] 6.1 Verify Discord parent summaries use `send_message_with_reply` against the original normal-channel message.
- [x] 6.2 Verify Slack parent summaries route to the original parent channel/message context without posting follow-up thread turns back to the parent channel.
- [x] 6.3 Ensure parent summary failures are logged separately and do not modify child thread session state.

## 7. Tests And Verification

- [x] 7.1 Add configuration parsing and validation tests for disabled defaults and invalid enable combinations.
- [x] 7.2 Add handoff broker tests proving completion metadata is attached only when configured.
- [x] 7.3 Add dispatcher tests proving only the initial handoff child turn triggers a parent summary reply.
- [x] 7.4 Add tests proving follow-up messages inside the child thread do not notify the parent channel.
- [x] 7.5 Add tests proving tool/thinking content is removed before summarization.
- [x] 7.6 Add failure-path tests for child failure, summary failure, and parent reply send failure.
- [x] 7.7 Run `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test`.
