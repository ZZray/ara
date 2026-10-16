# Connection-local message queue

The workbench's composer queue projects the compact Node's existing FIFO. It is
not a cloud calendar, a persistent Job, or a second scheduler. At most sixteen
pending messages share the active native Session, execution mode, model binding
and Goal authority; changing a message edits text only. A successful Run does
not accept its Goal or grant additional background time.

`agent-queue-v1` distinguishes managed Nodes from older peers. Queue identity is
the controller-issued Run ID. Management commands carry the explicit Session,
connection queue epoch, expected revision and current Run ID. The transport actor
alone removes, edits, reorders or transfers a pending message to steering. A
dequeued Run cannot be edited or deleted through the queue. Rejected commands
return an error and an authoritative snapshot without retrying their effects.

The browser waits for native acknowledgement before removing a row. A sent
WebSocket frame is not that acknowledgement. Unknown delivery triggers only a
state query; it does not replay a message or a mutation. New Run handover requires
the same device, native Session, connection generation and a previously observed
pending ID. Old terminal events cannot finish the new stream.

Steering has separate submitted, applied and unapplied receipts. Applied means
the Host admitted sourced text into the current execution context at a tool
boundary. It does not establish that the model acted on it. A Run ending before
admission must report unapplied; the Host never silently reruns that text as a
follow-up. Controller detach, unpair, abort and connection replacement clear
this temporary queue. Refresh is not a durable-recovery contract.

Scope and executable evidence: [0.2.2 queue delivery](../evidence/message-queue-0.2.2-20261016.md).
