# recognizer

Optional Python worker for face and pet recognition (phase 4+). It is started
by the Rust core as a child process and speaks JSON lines over stdin/stdout:
image path in, bounding boxes and embeddings out. It never touches the
database. The protocol will be specified in `docs/protocol.md`.
