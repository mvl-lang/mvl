# bzip

Pure bzip2 compression pipeline — demonstrates **Req 7 effect boundaries** with zero effects in core logic.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Pure functions | `fn compress_bytes(data: List[Int]) -> Result[...]` | No `!` annotation — fully testable |
| Effect boundary | `fn main() -> Unit ! Console` | Only entry point has effects |
| Result types | `Result[List[Int], CompressError]` | All failure modes explicit |
| Multi-module | `use rle::rle_encode` | Pipeline stages in separate files |

---

## Pipeline stages

```
compress:   input → RLE → BWT → MTF → Huffman → bitstream
decompress: bitstream → Huffman → MTF → BWT → RLE → output
```

| File | Stage | Algorithm |
|------|-------|-----------|
| `rle.mvl` | Run-length encoding | Compress repeated bytes |
| `bwt.mvl` | Burrows-Wheeler transform | Group similar characters |
| `mtf.mvl` | Move-to-front | Convert to small integers |
| `huffman.mvl` | Huffman coding | Variable-length encoding |
| `bitstream.mvl` | Bit packing | Pack codes into bytes |

---

## Effect boundary check

```bash
grep '!' examples/bzip/*.mvl
# Only main.mvl appears — all other files are pure
```

---

## Running

```bash
make build
cd examples/bzip
make test
```

---

## Related

- Spec: `.openspec/specs/002-effect-system/spec.md`
- Req 7: Effect tracking
