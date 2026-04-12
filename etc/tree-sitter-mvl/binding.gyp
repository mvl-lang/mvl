{
  "targets": [
    {
      "target_name": "tree_sitter_mvl_binding",
      "include_dirs": [
        "<!(node -e \"console.log(require('node-addon-api').include_dir)\")",
        "src"
      ],
      "defines": ["NAPI_DISABLE_CPP_EXCEPTIONS"],
      "sources": [
        "src/parser.c",
        "bindings/node/binding.cc"
      ],
      "cflags_c": [
        "-std=c99",
        "-fvisibility=hidden"
      ]
    }
  ]
}
