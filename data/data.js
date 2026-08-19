window.BENCHMARK_DATA = {
  "lastUpdate": 1787107983463,
  "repoUrl": "https://github.com/mvl-lang/mvl",
  "entries": {
    "Benchmark": [
      {
        "commit": {
          "author": {
            "name": "Ilja Heitlager",
            "username": "iheitlager",
            "email": "iheitlager@schubergphilis.com"
          },
          "committer": {
            "name": "Ilja Heitlager",
            "username": "iheitlager",
            "email": "iheitlager@schubergphilis.com"
          },
          "id": "2351ca0f5b9626657d96681bb97199fe9f9b8fa2",
          "message": "docs(changelog): record the validation remediation under 1.8.1\n\nThe v1.8.1 tag is being moved forward to include PR #2288, so the release\nnow contains that work — its own changelog section should say so.\n\nAlso corrects the section date 2026-08-12 -> 2026-08-14, the date the\nrelease was actually cut.",
          "timestamp": "2026-08-14T15:06:20Z",
          "url": "https://github.com/mvl-lang/mvl/commit/2351ca0f5b9626657d96681bb97199fe9f9b8fa2"
        },
        "date": 1787034526881,
        "tool": "cargo",
        "benches": [
          {
            "name": "layer/l1_literal",
            "value": 39276,
            "range": "± 126",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l1_subsume",
            "value": 42701,
            "range": "± 763",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l2_interval",
            "value": 46836,
            "range": "± 641",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l2_range",
            "value": 42597,
            "range": "± 553",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l3_symbolic",
            "value": 12965888,
            "range": "± 130331",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l4_cooper",
            "value": 38323,
            "range": "± 341",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l5_z3",
            "value": 47669,
            "range": "± 1613",
            "unit": "ns/iter"
          },
          {
            "name": "mode/type_alias/layered",
            "value": 15846292,
            "range": "± 115584",
            "unit": "ns/iter"
          },
          {
            "name": "mode/type_alias/fast-only",
            "value": 8019754,
            "range": "± 813587",
            "unit": "ns/iter"
          },
          {
            "name": "mode/type_alias/z3-only",
            "value": 70976878,
            "range": "± 2679451",
            "unit": "ns/iter"
          },
          {
            "name": "mode/struct_invariant/layered",
            "value": 140734,
            "range": "± 650",
            "unit": "ns/iter"
          },
          {
            "name": "mode/struct_invariant/fast-only",
            "value": 141048,
            "range": "± 1148",
            "unit": "ns/iter"
          },
          {
            "name": "mode/struct_invariant/z3-only",
            "value": 18850851,
            "range": "± 165026",
            "unit": "ns/iter"
          },
          {
            "name": "mode/contracts_requires/layered",
            "value": 138212,
            "range": "± 4319",
            "unit": "ns/iter"
          },
          {
            "name": "mode/contracts_requires/fast-only",
            "value": 138178,
            "range": "± 3555",
            "unit": "ns/iter"
          },
          {
            "name": "mode/contracts_requires/z3-only",
            "value": 73493976,
            "range": "± 223940",
            "unit": "ns/iter"
          },
          {
            "name": "corpus/type_alias",
            "value": 15387420,
            "range": "± 74283",
            "unit": "ns/iter"
          },
          {
            "name": "corpus/struct_invariant",
            "value": 141441,
            "range": "± 8753",
            "unit": "ns/iter"
          },
          {
            "name": "corpus/contracts_requires",
            "value": 137980,
            "range": "± 569",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "Ilja Heitlager",
            "username": "iheitlager",
            "email": "iheitlager@schubergphilis.com"
          },
          "committer": {
            "name": "Ilja Heitlager",
            "username": "iheitlager",
            "email": "iheitlager@schubergphilis.com"
          },
          "id": "2351ca0f5b9626657d96681bb97199fe9f9b8fa2",
          "message": "docs(changelog): record the validation remediation under 1.8.1\n\nThe v1.8.1 tag is being moved forward to include PR #2288, so the release\nnow contains that work — its own changelog section should say so.\n\nAlso corrects the section date 2026-08-12 -> 2026-08-14, the date the\nrelease was actually cut.",
          "timestamp": "2026-08-14T15:06:20Z",
          "url": "https://github.com/mvl-lang/mvl/commit/2351ca0f5b9626657d96681bb97199fe9f9b8fa2"
        },
        "date": 1787107982344,
        "tool": "cargo",
        "benches": [
          {
            "name": "layer/l1_literal",
            "value": 39544,
            "range": "± 263",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l1_subsume",
            "value": 43907,
            "range": "± 922",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l2_interval",
            "value": 48098,
            "range": "± 314",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l2_range",
            "value": 43495,
            "range": "± 588",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l3_symbolic",
            "value": 12528999,
            "range": "± 117873",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l4_cooper",
            "value": 40200,
            "range": "± 305",
            "unit": "ns/iter"
          },
          {
            "name": "layer/l5_z3",
            "value": 49770,
            "range": "± 1029",
            "unit": "ns/iter"
          },
          {
            "name": "mode/type_alias/layered",
            "value": 15946204,
            "range": "± 731567",
            "unit": "ns/iter"
          },
          {
            "name": "mode/type_alias/fast-only",
            "value": 7635228,
            "range": "± 431611",
            "unit": "ns/iter"
          },
          {
            "name": "mode/type_alias/z3-only",
            "value": 75026930,
            "range": "± 2008612",
            "unit": "ns/iter"
          },
          {
            "name": "mode/struct_invariant/layered",
            "value": 149236,
            "range": "± 1905",
            "unit": "ns/iter"
          },
          {
            "name": "mode/struct_invariant/fast-only",
            "value": 149606,
            "range": "± 8117",
            "unit": "ns/iter"
          },
          {
            "name": "mode/struct_invariant/z3-only",
            "value": 19773041,
            "range": "± 244938",
            "unit": "ns/iter"
          },
          {
            "name": "mode/contracts_requires/layered",
            "value": 143979,
            "range": "± 756",
            "unit": "ns/iter"
          },
          {
            "name": "mode/contracts_requires/fast-only",
            "value": 143752,
            "range": "± 834",
            "unit": "ns/iter"
          },
          {
            "name": "mode/contracts_requires/z3-only",
            "value": 73619167,
            "range": "± 2229116",
            "unit": "ns/iter"
          },
          {
            "name": "corpus/type_alias",
            "value": 16419033,
            "range": "± 398515",
            "unit": "ns/iter"
          },
          {
            "name": "corpus/struct_invariant",
            "value": 149066,
            "range": "± 3389",
            "unit": "ns/iter"
          },
          {
            "name": "corpus/contracts_requires",
            "value": 144141,
            "range": "± 817",
            "unit": "ns/iter"
          }
        ]
      }
    ]
  }
}