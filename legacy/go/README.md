# Legacy Go Djinn

This folder contains the original Go implementation of Djinn.

It is kept as a working reference while the root project moves toward a Rust
workspace. The legacy implementation includes the Bubble Tea picker, dotfile tag
scanner, editor open mode, and JSON cache generation.

Build it directly with:

```bash
make -C legacy/go build
```
