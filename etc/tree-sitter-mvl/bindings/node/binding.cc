#include "napi.h"
#include "tree_sitter/parser.h"

using namespace Napi;

extern "C" TSLanguage *tree_sitter_mvl();

Object Init(Env env, Object exports) {
  exports["name"] = String::New(env, "mvl");
  exports["language"] = External<TSLanguage>::New(env, tree_sitter_mvl());
  return exports;
}

NODE_API_MODULE(tree_sitter_mvl_binding, Init)
