# LiteLLM usage tracker

Status: implemented; live deployment verification pending

This feature adds a `litellm` provider that reports per-user or per-key spend,
token counts, request counts, and configured budget state from a LiteLLM proxy.

- [Specification](spec.md)
- The first target deployment is `https://my-litellm-instance.example`, running
  LiteLLM Enterprise 1.100.0.
- The provider uses documented management endpoints and a dedicated virtual key.
- The Rust provider, CLI registration, mock-server coverage, and setup
  documentation are complete. A real LiteLLM virtual key is still needed to
  verify deployment-specific permissions.
