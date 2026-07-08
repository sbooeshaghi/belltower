# API Contract

`openapi.yaml` is the committed Belltower HTTP/SSE contract for client
consumers.

Regenerate it from the Rust route metadata and `bt-protocol` schema
components with:

```bash
make openapi
```

Check that the committed file is current with:

```bash
make openapi-check
```

The file is JSON-formatted OpenAPI 3.1 stored with a `.yaml` extension;
JSON is valid YAML and keeps generation dependency-free.
