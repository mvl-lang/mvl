; ModuleID = 'net_main'
source_filename = "net_main"
target triple = "arm64-apple-darwin24.6.0"

%Request = type { i64, i8, i8 }
%DbPoolState = type { i64 }
%RequestHandlerState = type { ptr }
%TestClientState = type { i64 }
%QueryResult = type { ptr }

@str_lit = private unnamed_addr constant [10 x i8] c"127.0.0.1\00", align 1
@str_lit.1 = private unnamed_addr constant [2 x i8] c" \00", align 1
@str_lit.2 = private unnamed_addr constant [31 x i8] c" HTTP/1.0\0D\0AHost: localhost\0D\0A\0D\0A\00", align 1
@str_lit.3 = private unnamed_addr constant [26 x i8] c"TestClient: write error: \00", align 1
@printf_fmt = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.4 = private unnamed_addr constant [28 x i8] c"TestClient: connect error: \00", align 1
@printf_fmt.5 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.6 = private unnamed_addr constant [16 x i8] c"DbPool: query '\00", align 1
@str_lit.7 = private unnamed_addr constant [12 x i8] c"' for req #\00", align 1
@int_fmt = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@printf_fmt.8 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.9 = private unnamed_addr constant [29 x i8] c"[{name: Alice}, {name: Bob}]\00", align 1
@str_lit.10 = private unnamed_addr constant [18 x i8] c"Handler: routing \00", align 1
@str_lit.11 = private unnamed_addr constant [8 x i8] c" (req #\00", align 1
@int_fmt.12 = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@str_lit.13 = private unnamed_addr constant [2 x i8] c")\00", align 1
@printf_fmt.14 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.15 = private unnamed_addr constant [23 x i8] c"Handler: 200 OK (req #\00", align 1
@int_fmt.16 = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@str_lit.17 = private unnamed_addr constant [9 x i8] c") \E2\80\94 ok\00", align 1
@str_lit.18 = private unnamed_addr constant [3 x i8] c"ok\00", align 1
@str_lit.19 = private unnamed_addr constant [30 x i8] c"Handler: 404 Not Found (req #\00", align 1
@int_fmt.20 = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@str_lit.21 = private unnamed_addr constant [2 x i8] c")\00", align 1
@str_lit.22 = private unnamed_addr constant [23 x i8] c"Handler: 200 OK (req #\00", align 1
@int_fmt.23 = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@str_lit.24 = private unnamed_addr constant [7 x i8] c") \E2\80\94 \00", align 1
@str_lit.25 = private unnamed_addr constant [1 x i8] zeroinitializer, align 1
@str_lit.26 = private unnamed_addr constant [4 x i8] c"GET\00", align 1
@str_lit.27 = private unnamed_addr constant [5 x i8] c"POST\00", align 1
@str_lit.28 = private unnamed_addr constant [7 x i8] c"DELETE\00", align 1
@str_lit.29 = private unnamed_addr constant [8 x i8] c"UNKNOWN\00", align 1
@str_lit.30 = private unnamed_addr constant [7 x i8] c"/users\00", align 1
@str_lit.31 = private unnamed_addr constant [8 x i8] c"/health\00", align 1
@str_lit.32 = private unnamed_addr constant [9 x i8] c"/unknown\00", align 1
@str_lit.33 = private unnamed_addr constant [6 x i8] c"Users\00", align 1
@str_lit.34 = private unnamed_addr constant [7 x i8] c"Health\00", align 1
@str_lit.35 = private unnamed_addr constant [8 x i8] c"Unknown\00", align 1
@str_lit.36 = private unnamed_addr constant [34 x i8] c"HTTP/1.0 200 OK\0D\0AContent-Length: \00", align 1
@int_fmt.37 = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@str_lit.38 = private unnamed_addr constant [5 x i8] c"\0D\0A\0D\0A\00", align 1
@str_lit.39 = private unnamed_addr constant [46 x i8] c"HTTP/1.0 404 Not Found\0D\0AContent-Length: 0\0D\0A\0D\0A\00", align 1
@printf_fmt.40 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.41 = private unnamed_addr constant [23 x i8] c"Handler: write error: \00", align 1
@printf_fmt.42 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.43 = private unnamed_addr constant [18 x i8] c"Server: accepted \00", align 1
@str_lit.44 = private unnamed_addr constant [2 x i8] c" \00", align 1
@str_lit.45 = private unnamed_addr constant [8 x i8] c" (req #\00", align 1
@int_fmt.46 = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@str_lit.47 = private unnamed_addr constant [2 x i8] c")\00", align 1
@printf_fmt.48 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.49 = private unnamed_addr constant [28 x i8] c"Server: read error on req #\00", align 1
@int_fmt.50 = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@str_lit.51 = private unnamed_addr constant [3 x i8] c": \00", align 1
@printf_fmt.52 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.53 = private unnamed_addr constant [30 x i8] c"Server: accept error on req #\00", align 1
@int_fmt.54 = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@str_lit.55 = private unnamed_addr constant [3 x i8] c": \00", align 1
@printf_fmt.56 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.57 = private unnamed_addr constant [10 x i8] c"127.0.0.1\00", align 1
@str_lit.58 = private unnamed_addr constant [24 x i8] c"Server: listen failed: \00", align 1
@printf_fmt.59 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.60 = private unnamed_addr constant [21 x i8] c"Server: port error: \00", align 1
@printf_fmt.61 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.62 = private unnamed_addr constant [27 x i8] c"Server: listening on port \00", align 1
@int_fmt.63 = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@printf_fmt.64 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.65 = private unnamed_addr constant [4 x i8] c"GET\00", align 1
@str_lit.66 = private unnamed_addr constant [7 x i8] c"/users\00", align 1
@str_lit.67 = private unnamed_addr constant [4 x i8] c"GET\00", align 1
@str_lit.68 = private unnamed_addr constant [8 x i8] c"/health\00", align 1
@str_lit.69 = private unnamed_addr constant [7 x i8] c"DELETE\00", align 1
@str_lit.70 = private unnamed_addr constant [7 x i8] c"/admin\00", align 1
@println_fmt = private unnamed_addr constant [33 x i8] c"Server: all requests processed.\0A\00", align 1

declare void @println(ptr)

declare void @print(ptr)

declare void @eprintln(ptr)

declare void @eprint(ptr)

declare ptr @format(ptr)

declare void @assert(i1)

declare void @assert_eq(ptr, ptr)

declare i64 @panic(ptr)

define ptr @range(i64 %start, i64 %end) {
entry:
  %start1 = alloca i64, align 8
  store i64 %start, ptr %start1, align 4
  %end2 = alloca i64, align 8
  store i64 %end, ptr %end2, align 4
  %arr_new = call ptr @mvl_array_new(i64 8, i64 4)
  %result = alloca ptr, align 8
  store ptr %arr_new, ptr %result, align 8
  %start3 = load i64, ptr %start1, align 4
  %current = alloca i64, align 8
  store i64 %start3, ptr %current, align 4
  br label %while_cond

while_cond:                                       ; preds = %ok, %entry
  %current4 = load i64, ptr %current, align 4
  %end5 = load i64, ptr %end2, align 4
  %lt = icmp slt i64 %current4, %end5
  br i1 %lt, label %while_body, label %while_exit

while_body:                                       ; preds = %while_cond
  %result6 = load ptr, ptr %result, align 8
  %current7 = load i64, ptr %current, align 4
  %push_slot = alloca i64, align 8
  store i64 %current7, ptr %push_slot, align 4
  call void @mvl_array_push(ptr %result6, ptr %push_slot)
  %current8 = load i64, ptr %current, align 4
  %add = call { i64, i1 } @llvm.sadd.with.overflow.i64(i64 %current8, i64 1)
  %add_val = extractvalue { i64, i1 } %add, 0
  %add_ovf = extractvalue { i64, i1 } %add, 1
  br i1 %add_ovf, label %overflow, label %ok

while_exit:                                       ; preds = %while_cond
  %result9 = load ptr, ptr %result, align 8
  ret ptr %result9

overflow:                                         ; preds = %while_body
  call void @llvm.trap()
  unreachable

ok:                                               ; preds = %while_body
  store i64 %add_val, ptr %current, align 4
  br label %while_cond
}

define i64 @str_len(ptr %0) {
entry:
  ret i64 0
}

define ptr @str_chars(ptr %0) {
entry:
  %chars = call ptr @mvl_string_chars(ptr %0)
  ret ptr %chars
}

define ptr @str_char_at(ptr %0, i64 %1) {
entry:
  ret ptr null
}

define ptr @str_from_chars(ptr %0) {
entry:
  ret ptr null
}

define i8 @str_byte_at(ptr %0, i64 %1) {
entry:
  ret i8 0
}

define ptr @str_from_bytes(ptr %0) {
entry:
  ret ptr null
}

define ptr @str_concat(ptr %0, ptr %1) {
entry:
  %concat = call ptr @mvl_string_concat(ptr %0, ptr %1)
  ret ptr %concat
}

define ptr @str_trim(ptr %0) {
entry:
  ret ptr null
}

define ptr @str_to_upper(ptr %0) {
entry:
  ret ptr null
}

define ptr @str_to_lower(ptr %0) {
entry:
  ret ptr null
}

define i1 @str_starts_with(ptr %0, ptr %1) {
entry:
  ret i1 false
}

define i1 @str_ends_with(ptr %0, ptr %1) {
entry:
  ret i1 false
}

define { i8, ptr } @str_find(ptr %0, ptr %1) {
entry:
  ret { i8, ptr } { i8 1, ptr null }
}

define ptr @str_replace(ptr %0, ptr %1, ptr %2) {
entry:
  ret ptr null
}

define ptr @str_split(ptr %0, ptr %1) {
entry:
  ret ptr null
}

define ptr @str_substring(ptr %0, i64 %1, i64 %2) {
entry:
  ret ptr null
}

define i1 @str_contains(ptr %0, ptr %1) {
entry:
  ret i1 false
}

define { i8, ptr } @str_parse_int(ptr %0) {
entry:
  ret { i8, ptr } { i8 1, ptr null }
}

define { i8, ptr } @str_parse_float(ptr %0) {
entry:
  ret { i8, ptr } { i8 1, ptr null }
}

define ptr @trim(ptr %s) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %s2 = load ptr, ptr %s1, align 8
  %call = call ptr @str_trim(ptr %s2)
  ret ptr %call
}

define ptr @to_upper(ptr %s) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %s2 = load ptr, ptr %s1, align 8
  %call = call ptr @str_to_upper(ptr %s2)
  ret ptr %call
}

define ptr @to_lower(ptr %s) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %s2 = load ptr, ptr %s1, align 8
  %call = call ptr @str_to_lower(ptr %s2)
  ret ptr %call
}

define ptr @chars(ptr %s) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %s2 = load ptr, ptr %s1, align 8
  %call = call ptr @str_chars(ptr %s2)
  ret ptr %call
}

define ptr @concat(ptr %s, ptr %other) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %other2 = alloca ptr, align 8
  store ptr %other, ptr %other2, align 8
  %s3 = load ptr, ptr %s1, align 8
  %other4 = load ptr, ptr %other2, align 8
  %call = call ptr @str_concat(ptr %s3, ptr %other4)
  ret ptr %call
}

define i1 @starts_with(ptr %s, ptr %prefix) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %prefix2 = alloca ptr, align 8
  store ptr %prefix, ptr %prefix2, align 8
  %s3 = load ptr, ptr %s1, align 8
  %prefix4 = load ptr, ptr %prefix2, align 8
  %call = call i1 @str_starts_with(ptr %s3, ptr %prefix4)
  ret i1 %call
}

define i1 @ends_with(ptr %s, ptr %suffix) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %suffix2 = alloca ptr, align 8
  store ptr %suffix, ptr %suffix2, align 8
  %s3 = load ptr, ptr %s1, align 8
  %suffix4 = load ptr, ptr %suffix2, align 8
  %call = call i1 @str_ends_with(ptr %s3, ptr %suffix4)
  ret i1 %call
}

define { i8, ptr } @find(ptr %s, ptr %sub) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %sub2 = alloca ptr, align 8
  store ptr %sub, ptr %sub2, align 8
  %s3 = load ptr, ptr %s1, align 8
  %sub4 = load ptr, ptr %sub2, align 8
  %call = call { i8, ptr } @str_find(ptr %s3, ptr %sub4)
  ret { i8, ptr } %call
}

define ptr @replace(ptr %s, ptr %from, ptr %to) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %from2 = alloca ptr, align 8
  store ptr %from, ptr %from2, align 8
  %to3 = alloca ptr, align 8
  store ptr %to, ptr %to3, align 8
  %s4 = load ptr, ptr %s1, align 8
  %from5 = load ptr, ptr %from2, align 8
  %to6 = load ptr, ptr %to3, align 8
  %call = call ptr @str_replace(ptr %s4, ptr %from5, ptr %to6)
  ret ptr %call
}

define ptr @split(ptr %s, ptr %sep) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %sep2 = alloca ptr, align 8
  store ptr %sep, ptr %sep2, align 8
  %s3 = load ptr, ptr %s1, align 8
  %sep4 = load ptr, ptr %sep2, align 8
  %call = call ptr @str_split(ptr %s3, ptr %sep4)
  ret ptr %call
}

define ptr @substring(ptr %s, i64 %start, i64 %end) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %start2 = alloca i64, align 8
  store i64 %start, ptr %start2, align 4
  %end3 = alloca i64, align 8
  store i64 %end, ptr %end3, align 4
  %s4 = load ptr, ptr %s1, align 8
  %start5 = load i64, ptr %start2, align 4
  %end6 = load i64, ptr %end3, align 4
  %call = call ptr @str_substring(ptr %s4, i64 %start5, i64 %end6)
  ret ptr %call
}

define i64 @str_length(ptr %s) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %s2 = load ptr, ptr %s1, align 8
  %call = call i64 @str_len(ptr %s2)
  ret i64 %call
}

define i1 @str_is_empty(ptr %s) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %s2 = load ptr, ptr %s1, align 8
  %call = call i64 @str_len(ptr %s2)
  %eq = icmp eq i64 %call, 0
  ret i1 %eq
}

define i1 @str_contains_sub(ptr %s, ptr %sub) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %sub2 = alloca ptr, align 8
  store ptr %sub, ptr %sub2, align 8
  %s3 = load ptr, ptr %s1, align 8
  %sub4 = load ptr, ptr %sub2, align 8
  %call = call i1 @str_contains(ptr %s3, ptr %sub4)
  ret i1 %call
}

define { i8, ptr } @parse_int(ptr %s) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %s2 = load ptr, ptr %s1, align 8
  %call = call { i8, ptr } @str_parse_int(ptr %s2)
  ret { i8, ptr } %call
}

define { i8, ptr } @parse_float(ptr %s) {
entry:
  %s1 = alloca ptr, align 8
  store ptr %s, ptr %s1, align 8
  %s2 = load ptr, ptr %s1, align 8
  %call = call { i8, ptr } @str_parse_float(ptr %s2)
  ret { i8, ptr } %call
}

define i64 @list_len(ptr %0) {
entry:
  %list_len = call i64 @mvl_array_len(ptr %0)
  ret i64 %list_len
}

define { i8, ptr } @list_get(ptr %0, i64 %1) {
entry:
  %raw = call ptr @mvl_array_get(ptr %0, i64 %1)
  %is_null = icmp eq ptr %raw, null
  %some_ptr = insertvalue { i8, ptr } zeroinitializer, ptr %raw, 1
  %opt = select i1 %is_null, { i8, ptr } { i8 1, ptr null }, { i8, ptr } %some_ptr
  ret { i8, ptr } %opt
}

define ptr @list_push(ptr %0, i64 %1) {
entry:
  ret ptr null
}

define ptr @list_slice(ptr %0, i64 %1, i64 %2) {
entry:
  ret ptr null
}

define ptr @list_concat(ptr %0, ptr %1) {
entry:
  ret ptr null
}

define i1 @list_contains(ptr %0, i64 %1) {
entry:
  ret i1 false
}

define ptr @str_join(ptr %xs, ptr %sep) {
entry:
  %xs1 = alloca ptr, align 8
  store ptr %xs, ptr %xs1, align 8
  %sep2 = alloca ptr, align 8
  store ptr %sep, ptr %sep2, align 8
  %str_new = call ptr @mvl_string_new(ptr @str_lit.25, i64 0)
  %drop_xs = load ptr, ptr %xs1, align 8
  call void @mvl_string_ptr_array_drop(ptr %drop_xs)
  %drop_sep = load ptr, ptr %sep2, align 8
  call void @mvl_string_drop(ptr %drop_sep)
  ret ptr %str_new
}

define ptr @method_name(i8 %m) {
entry:
  %m1 = alloca i8, align 1
  store i8 %m, ptr %m1, align 1
  %m2 = load i8, ptr %m1, align 1
  switch i8 %m2, label %match_default [
    i8 0, label %arm0
    i8 1, label %arm1
    i8 2, label %arm2
    i8 3, label %arm3
  ]

match_merge:                                      ; preds = %arm3, %arm2, %arm1, %arm0
  %match_val = phi ptr [ %str_new, %arm0 ], [ %str_new3, %arm1 ], [ %str_new4, %arm2 ], [ %str_new5, %arm3 ]
  ret ptr %match_val

match_default:                                    ; preds = %entry
  unreachable

arm0:                                             ; preds = %entry
  %str_new = call ptr @mvl_string_new(ptr @str_lit.26, i64 3)
  br label %match_merge

arm1:                                             ; preds = %entry
  %str_new3 = call ptr @mvl_string_new(ptr @str_lit.27, i64 4)
  br label %match_merge

arm2:                                             ; preds = %entry
  %str_new4 = call ptr @mvl_string_new(ptr @str_lit.28, i64 6)
  br label %match_merge

arm3:                                             ; preds = %entry
  %str_new5 = call ptr @mvl_string_new(ptr @str_lit.29, i64 7)
  br label %match_merge
}

define ptr @route_path(i8 %r) {
entry:
  %r1 = alloca i8, align 1
  store i8 %r, ptr %r1, align 1
  %r2 = load i8, ptr %r1, align 1
  switch i8 %r2, label %match_default [
    i8 0, label %arm0
    i8 1, label %arm1
    i8 2, label %arm2
  ]

match_merge:                                      ; preds = %arm2, %arm1, %arm0
  %match_val = phi ptr [ %str_new, %arm0 ], [ %str_new3, %arm1 ], [ %str_new4, %arm2 ]
  ret ptr %match_val

match_default:                                    ; preds = %entry
  unreachable

arm0:                                             ; preds = %entry
  %str_new = call ptr @mvl_string_new(ptr @str_lit.30, i64 6)
  br label %match_merge

arm1:                                             ; preds = %entry
  %str_new3 = call ptr @mvl_string_new(ptr @str_lit.31, i64 7)
  br label %match_merge

arm2:                                             ; preds = %entry
  %str_new4 = call ptr @mvl_string_new(ptr @str_lit.32, i64 8)
  br label %match_merge
}

define ptr @route_name(i8 %r) {
entry:
  %r1 = alloca i8, align 1
  store i8 %r, ptr %r1, align 1
  %r2 = load i8, ptr %r1, align 1
  switch i8 %r2, label %match_default [
    i8 0, label %arm0
    i8 1, label %arm1
    i8 2, label %arm2
  ]

match_merge:                                      ; preds = %arm2, %arm1, %arm0
  %match_val = phi ptr [ %str_new, %arm0 ], [ %str_new3, %arm1 ], [ %str_new4, %arm2 ]
  ret ptr %match_val

match_default:                                    ; preds = %entry
  unreachable

arm0:                                             ; preds = %entry
  %str_new = call ptr @mvl_string_new(ptr @str_lit.33, i64 5)
  br label %match_merge

arm1:                                             ; preds = %entry
  %str_new3 = call ptr @mvl_string_new(ptr @str_lit.34, i64 6)
  br label %match_merge

arm2:                                             ; preds = %entry
  %str_new4 = call ptr @mvl_string_new(ptr @str_lit.35, i64 7)
  br label %match_merge
}

define ptr @http_ok(ptr %body) {
entry:
  %body1 = alloca ptr, align 8
  store ptr %body, ptr %body1, align 8
  %str_new = call ptr @mvl_string_new(ptr @str_lit.36, i64 33)
  %body2 = load ptr, ptr %body1, align 8
  %coll_len = call i64 @mvl_string_len(ptr %body2)
  %int_str_buf = alloca [32 x i8], align 1
  %snprintf_int = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %int_str_buf, i64 32, ptr @int_fmt.37, i64 %coll_len)
  %str_len = zext i32 %snprintf_int to i64
  %str_new3 = call ptr @mvl_string_new(ptr %int_str_buf, i64 %str_len)
  %str_concat = call ptr @mvl_string_concat(ptr %str_new, ptr %str_new3)
  %str_new4 = call ptr @mvl_string_new(ptr @str_lit.38, i64 4)
  %str_concat5 = call ptr @mvl_string_concat(ptr %str_concat, ptr %str_new4)
  %body6 = load ptr, ptr %body1, align 8
  %str_concat7 = call ptr @mvl_string_concat(ptr %str_concat5, ptr %body6)
  %drop_body = load ptr, ptr %body1, align 8
  call void @mvl_string_drop(ptr %drop_body)
  ret ptr %str_concat7
}

define ptr @http_not_found() {
entry:
  %str_new = call ptr @mvl_string_new(ptr @str_lit.39, i64 45)
  ret ptr %str_new
}

define %Request @request_by_id(i64 %id) {
entry:
  %id1 = alloca i64, align 8
  store i64 %id, ptr %id1, align 4
  %id2 = load i64, ptr %id1, align 4
  %eq = icmp eq i64 %id2, 1
  br i1 %eq, label %then, label %else

then:                                             ; preds = %entry
  %struct_tmp = alloca %Request, align 8
  %id3 = load i64, ptr %id1, align 4
  %f0_ptr = getelementptr inbounds nuw %Request, ptr %struct_tmp, i32 0, i32 0
  store i64 %id3, ptr %f0_ptr, align 4
  %f1_ptr = getelementptr inbounds nuw %Request, ptr %struct_tmp, i32 0, i32 1
  store i8 0, ptr %f1_ptr, align 1
  %f2_ptr = getelementptr inbounds nuw %Request, ptr %struct_tmp, i32 0, i32 2
  store i8 0, ptr %f2_ptr, align 1
  %struct_val = load %Request, ptr %struct_tmp, align 4
  br label %merge

merge:                                            ; preds = %merge7, %then
  %if_val21 = phi %Request [ %struct_val, %then ], [ %if_val, %merge7 ]
  ret %Request %if_val21

else:                                             ; preds = %entry
  %id4 = load i64, ptr %id1, align 4
  %eq5 = icmp eq i64 %id4, 2
  br i1 %eq5, label %then6, label %else8

then6:                                            ; preds = %else
  %struct_tmp9 = alloca %Request, align 8
  %id10 = load i64, ptr %id1, align 4
  %f0_ptr11 = getelementptr inbounds nuw %Request, ptr %struct_tmp9, i32 0, i32 0
  store i64 %id10, ptr %f0_ptr11, align 4
  %f1_ptr12 = getelementptr inbounds nuw %Request, ptr %struct_tmp9, i32 0, i32 1
  store i8 0, ptr %f1_ptr12, align 1
  %f2_ptr13 = getelementptr inbounds nuw %Request, ptr %struct_tmp9, i32 0, i32 2
  store i8 1, ptr %f2_ptr13, align 1
  %struct_val14 = load %Request, ptr %struct_tmp9, align 4
  br label %merge7

merge7:                                           ; preds = %else8, %then6
  %if_val = phi %Request [ %struct_val14, %then6 ], [ %struct_val20, %else8 ]
  br label %merge

else8:                                            ; preds = %else
  %struct_tmp15 = alloca %Request, align 8
  %id16 = load i64, ptr %id1, align 4
  %f0_ptr17 = getelementptr inbounds nuw %Request, ptr %struct_tmp15, i32 0, i32 0
  store i64 %id16, ptr %f0_ptr17, align 4
  %f1_ptr18 = getelementptr inbounds nuw %Request, ptr %struct_tmp15, i32 0, i32 1
  store i8 2, ptr %f1_ptr18, align 1
  %f2_ptr19 = getelementptr inbounds nuw %Request, ptr %struct_tmp15, i32 0, i32 2
  store i8 2, ptr %f2_ptr19, align 1
  %struct_val20 = load %Request, ptr %struct_tmp15, align 4
  br label %merge7
}

define void @write_and_close(ptr %stream, ptr %resp, ptr %label) {
entry:
  %stream1 = alloca ptr, align 8
  store ptr %stream, ptr %stream1, align 8
  %resp2 = alloca ptr, align 8
  store ptr %resp, ptr %resp2, align 8
  %label3 = alloca ptr, align 8
  store ptr %label, ptr %label3, align 8
  %stream4 = load ptr, ptr %stream1, align 8
  %resp5 = load ptr, ptr %resp2, align 8
  %io_c_call = call { i8, ptr } @_mvl_net_tcp_write(ptr %stream4, ptr %resp5)
  %c_disc = extractvalue { i8, ptr } %io_c_call, 0
  %c_direct = extractvalue { i8, ptr } %io_c_call, 1
  %c_slot = alloca ptr, align 8
  store ptr %c_direct, ptr %c_slot, align 8
  %c_wrapped = alloca { i8, ptr }, align 8
  %c_disc_ptr = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped, i32 0, i32 0
  store i8 %c_disc, ptr %c_disc_ptr, align 1
  %c_payload_ptr = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped, i32 0, i32 1
  store ptr %c_slot, ptr %c_payload_ptr, align 8
  %c_result = load { i8, ptr }, ptr %c_wrapped, align 8
  %disc = extractvalue { i8, ptr } %c_result, 0
  switch i8 %disc, label %match_default [
    i8 0, label %arm0
    i8 1, label %arm1
  ]

match_merge:                                      ; preds = %arm1, %arm0
  %match_val = phi i8 [ 0, %arm0 ], [ 0, %arm1 ]
  %drop_resp = load ptr, ptr %resp2, align 8
  call void @mvl_string_drop(ptr %drop_resp)
  %drop_label = load ptr, ptr %label3, align 8
  call void @mvl_string_drop(ptr %drop_label)
  ret void

match_default:                                    ; preds = %entry
  unreachable

arm0:                                             ; preds = %entry
  %label6 = load ptr, ptr %label3, align 8
  %str_cptr = call ptr @mvl_string_ptr(ptr %label6)
  %println = call i32 (ptr, ...) @printf(ptr @printf_fmt.40, ptr %str_cptr)
  %stream7 = load ptr, ptr %stream1, align 8
  call void @_mvl_net_tcp_close_stream(ptr %stream7)
  br label %match_merge

arm1:                                             ; preds = %entry
  %payload_ptr = extractvalue { i8, ptr } %c_result, 1
  %e = load ptr, ptr %payload_ptr, align 8
  %e8 = alloca ptr, align 8
  store ptr %e, ptr %e8, align 8
  %str_new = call ptr @mvl_string_new(ptr @str_lit.41, i64 22)
  %e9 = load ptr, ptr %e8, align 8
  %str_concat = call ptr @mvl_string_concat(ptr %str_new, ptr %e9)
  %str_cptr10 = call ptr @mvl_string_ptr(ptr %str_concat)
  %println11 = call i32 (ptr, ...) @printf(ptr @printf_fmt.42, ptr %str_cptr10)
  %stream12 = load ptr, ptr %stream1, align 8
  call void @_mvl_net_tcp_close_stream(ptr %stream12)
  br label %match_merge
}

define void @accept_request(ptr %listener, i64 %id, ptr %handler) {
entry:
  %listener1 = alloca ptr, align 8
  store ptr %listener, ptr %listener1, align 8
  %id2 = alloca i64, align 8
  store i64 %id, ptr %id2, align 4
  %handler3 = alloca ptr, align 8
  store ptr %handler, ptr %handler3, align 8
  %listener4 = load ptr, ptr %listener1, align 8
  %io_c_call = call { i8, ptr } @_mvl_net_tcp_accept(ptr %listener4)
  %c_disc = extractvalue { i8, ptr } %io_c_call, 0
  %c_direct = extractvalue { i8, ptr } %io_c_call, 1
  %c_slot = alloca ptr, align 8
  store ptr %c_direct, ptr %c_slot, align 8
  %c_wrapped = alloca { i8, ptr }, align 8
  %c_disc_ptr = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped, i32 0, i32 0
  store i8 %c_disc, ptr %c_disc_ptr, align 1
  %c_payload_ptr = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped, i32 0, i32 1
  store ptr %c_slot, ptr %c_payload_ptr, align 8
  %c_result = load { i8, ptr }, ptr %c_wrapped, align 8
  %disc = extractvalue { i8, ptr } %c_result, 0
  switch i8 %disc, label %match_default [
    i8 0, label %arm0
    i8 1, label %arm1
  ]

match_merge:                                      ; preds = %arm1, %match_merge16
  ret void

match_default:                                    ; preds = %entry
  unreachable

arm0:                                             ; preds = %entry
  %payload_ptr = extractvalue { i8, ptr } %c_result, 1
  %stream = load ptr, ptr %payload_ptr, align 8
  %stream5 = alloca ptr, align 8
  store ptr %stream, ptr %stream5, align 8
  %stream6 = load ptr, ptr %stream5, align 8
  %io_c_call7 = call { i8, ptr } @_mvl_net_tcp_read(ptr %stream6)
  %c_disc8 = extractvalue { i8, ptr } %io_c_call7, 0
  %c_direct9 = extractvalue { i8, ptr } %io_c_call7, 1
  %c_slot10 = alloca ptr, align 8
  store ptr %c_direct9, ptr %c_slot10, align 8
  %c_wrapped11 = alloca { i8, ptr }, align 8
  %c_disc_ptr12 = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped11, i32 0, i32 0
  store i8 %c_disc8, ptr %c_disc_ptr12, align 1
  %c_payload_ptr13 = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped11, i32 0, i32 1
  store ptr %c_slot10, ptr %c_payload_ptr13, align 8
  %c_result14 = load { i8, ptr }, ptr %c_wrapped11, align 8
  %disc15 = extractvalue { i8, ptr } %c_result14, 0
  switch i8 %disc15, label %match_default17 [
    i8 0, label %arm018
    i8 1, label %arm119
  ]

arm1:                                             ; preds = %entry
  %payload_ptr56 = extractvalue { i8, ptr } %c_result, 1
  %e57 = load ptr, ptr %payload_ptr56, align 8
  %e58 = alloca ptr, align 8
  store ptr %e57, ptr %e58, align 8
  %str_new59 = call ptr @mvl_string_new(ptr @str_lit.53, i64 29)
  %id60 = load i64, ptr %id2, align 4
  %int_str_buf61 = alloca [32 x i8], align 1
  %snprintf_int62 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %int_str_buf61, i64 32, ptr @int_fmt.54, i64 %id60)
  %str_len63 = zext i32 %snprintf_int62 to i64
  %str_new64 = call ptr @mvl_string_new(ptr %int_str_buf61, i64 %str_len63)
  %str_concat65 = call ptr @mvl_string_concat(ptr %str_new59, ptr %str_new64)
  %str_new66 = call ptr @mvl_string_new(ptr @str_lit.55, i64 2)
  %str_concat67 = call ptr @mvl_string_concat(ptr %str_concat65, ptr %str_new66)
  %e68 = load ptr, ptr %e58, align 8
  %str_concat69 = call ptr @mvl_string_concat(ptr %str_concat67, ptr %e68)
  %str_cptr70 = call ptr @mvl_string_ptr(ptr %str_concat69)
  %println71 = call i32 (ptr, ...) @printf(ptr @printf_fmt.56, ptr %str_cptr70)
  br label %match_merge

match_merge16:                                    ; preds = %arm119, %arm018
  br label %match_merge

match_default17:                                  ; preds = %arm0
  unreachable

arm018:                                           ; preds = %arm0
  %payload_ptr20 = extractvalue { i8, ptr } %c_result14, 1
  %raw = load ptr, ptr %payload_ptr20, align 8
  %raw21 = alloca ptr, align 8
  store ptr %raw, ptr %raw21, align 8
  %id22 = load i64, ptr %id2, align 4
  %call = call %Request @request_by_id(i64 %id22)
  %req = alloca %Request, align 8
  store %Request %call, ptr %req, align 4
  %str_new = call ptr @mvl_string_new(ptr @str_lit.43, i64 17)
  %req23 = load %Request, ptr %req, align 4
  %method = extractvalue %Request %req23, 1
  %call24 = call ptr @method_name(i8 %method)
  %str_concat = call ptr @mvl_string_concat(ptr %str_new, ptr %call24)
  %str_new25 = call ptr @mvl_string_new(ptr @str_lit.44, i64 1)
  %str_concat26 = call ptr @mvl_string_concat(ptr %str_concat, ptr %str_new25)
  %req27 = load %Request, ptr %req, align 4
  %route = extractvalue %Request %req27, 2
  %call28 = call ptr @route_path(i8 %route)
  %str_concat29 = call ptr @mvl_string_concat(ptr %str_concat26, ptr %call28)
  %str_new30 = call ptr @mvl_string_new(ptr @str_lit.45, i64 7)
  %str_concat31 = call ptr @mvl_string_concat(ptr %str_concat29, ptr %str_new30)
  %id32 = load i64, ptr %id2, align 4
  %int_str_buf = alloca [32 x i8], align 1
  %snprintf_int = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %int_str_buf, i64 32, ptr @int_fmt.46, i64 %id32)
  %str_len = zext i32 %snprintf_int to i64
  %str_new33 = call ptr @mvl_string_new(ptr %int_str_buf, i64 %str_len)
  %str_concat34 = call ptr @mvl_string_concat(ptr %str_concat31, ptr %str_new33)
  %str_new35 = call ptr @mvl_string_new(ptr @str_lit.47, i64 1)
  %str_concat36 = call ptr @mvl_string_concat(ptr %str_concat34, ptr %str_new35)
  %str_cptr = call ptr @mvl_string_ptr(ptr %str_concat36)
  %println = call i32 (ptr, ...) @printf(ptr @printf_fmt.48, ptr %str_cptr)
  %handler37 = load ptr, ptr %handler3, align 8
  %actor_args = alloca [2 x i64], align 8
  %stream38 = load ptr, ptr %stream5, align 8
  %arg_i64 = ptrtoint ptr %stream38 to i64
  %arg_ptr_0 = getelementptr inbounds i64, ptr %actor_args, i64 0
  store i64 %arg_i64, ptr %arg_ptr_0, align 4
  %req39 = load %Request, ptr %req, align 4
  %arg_box = call ptr @mvl_box_new(i64 10)
  store %Request %req39, ptr %arg_box, align 4
  %arg_i6440 = ptrtoint ptr %arg_box to i64
  %arg_ptr_1 = getelementptr inbounds i64, ptr %actor_args, i64 1
  store i64 %arg_i6440, ptr %arg_ptr_1, align 4
  call void @mvl_actor_send(ptr %handler37, i64 0, i64 2, ptr %actor_args)
  br label %match_merge16

arm119:                                           ; preds = %arm0
  %payload_ptr41 = extractvalue { i8, ptr } %c_result14, 1
  %e = load ptr, ptr %payload_ptr41, align 8
  %e42 = alloca ptr, align 8
  store ptr %e, ptr %e42, align 8
  %str_new43 = call ptr @mvl_string_new(ptr @str_lit.49, i64 27)
  %id44 = load i64, ptr %id2, align 4
  %int_str_buf45 = alloca [32 x i8], align 1
  %snprintf_int46 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %int_str_buf45, i64 32, ptr @int_fmt.50, i64 %id44)
  %str_len47 = zext i32 %snprintf_int46 to i64
  %str_new48 = call ptr @mvl_string_new(ptr %int_str_buf45, i64 %str_len47)
  %str_concat49 = call ptr @mvl_string_concat(ptr %str_new43, ptr %str_new48)
  %str_new50 = call ptr @mvl_string_new(ptr @str_lit.51, i64 2)
  %str_concat51 = call ptr @mvl_string_concat(ptr %str_concat49, ptr %str_new50)
  %e52 = load ptr, ptr %e42, align 8
  %str_concat53 = call ptr @mvl_string_concat(ptr %str_concat51, ptr %e52)
  %str_cptr54 = call ptr @mvl_string_ptr(ptr %str_concat53)
  %println55 = call i32 (ptr, ...) @printf(ptr @printf_fmt.52, ptr %str_cptr54)
  br label %match_merge16
}

define i32 @main() {
entry:
  %str_new = call ptr @mvl_string_new(ptr @str_lit.57, i64 9)
  %net_c_call = call { i8, ptr } @_mvl_net_tcp_listen(ptr %str_new, i64 0)
  %c_disc = extractvalue { i8, ptr } %net_c_call, 0
  %c_direct = extractvalue { i8, ptr } %net_c_call, 1
  %c_slot = alloca ptr, align 8
  store ptr %c_direct, ptr %c_slot, align 8
  %c_wrapped = alloca { i8, ptr }, align 8
  %c_disc_ptr = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped, i32 0, i32 0
  store i8 %c_disc, ptr %c_disc_ptr, align 1
  %c_payload_ptr = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped, i32 0, i32 1
  store ptr %c_slot, ptr %c_payload_ptr, align 8
  %c_result = load { i8, ptr }, ptr %c_wrapped, align 8
  %disc = extractvalue { i8, ptr } %c_result, 0
  switch i8 %disc, label %match_default [
    i8 0, label %arm0
    i8 1, label %arm1
  ]

match_merge:                                      ; preds = %arm0
  %match_val = phi ptr [ %l2, %arm0 ]
  %listener = alloca ptr, align 8
  store ptr %match_val, ptr %listener, align 8
  %listener7 = load ptr, ptr %listener, align 8
  %net_port_call = call { i8, ptr } @_mvl_net_tcp_listener_port(ptr %listener7)
  %port_disc = extractvalue { i8, ptr } %net_port_call, 0
  %port_payload_ptr = extractvalue { i8, ptr } %net_port_call, 1
  %port_i64 = ptrtoint ptr %port_payload_ptr to i64
  %port_slot = alloca i64, align 8
  store i64 %port_i64, ptr %port_slot, align 4
  %port_result = alloca { i8, ptr }, align 8
  %port_disc_ptr = getelementptr inbounds nuw { i8, ptr }, ptr %port_result, i32 0, i32 0
  store i8 %port_disc, ptr %port_disc_ptr, align 1
  %port_payload_slot_ptr = getelementptr inbounds nuw { i8, ptr }, ptr %port_result, i32 0, i32 1
  store ptr %port_slot, ptr %port_payload_slot_ptr, align 8
  %port_wrapped = load { i8, ptr }, ptr %port_result, align 8
  %disc8 = extractvalue { i8, ptr } %port_wrapped, 0
  switch i8 %disc8, label %match_default10 [
    i8 0, label %arm011
    i8 1, label %arm112
  ]

match_default:                                    ; preds = %entry
  unreachable

arm0:                                             ; preds = %entry
  %payload_ptr = extractvalue { i8, ptr } %c_result, 1
  %l = load ptr, ptr %payload_ptr, align 8
  %l1 = alloca ptr, align 8
  store ptr %l, ptr %l1, align 8
  %l2 = load ptr, ptr %l1, align 8
  br label %match_merge

arm1:                                             ; preds = %entry
  %payload_ptr3 = extractvalue { i8, ptr } %c_result, 1
  %e = load ptr, ptr %payload_ptr3, align 8
  %e4 = alloca ptr, align 8
  store ptr %e, ptr %e4, align 8
  %str_new5 = call ptr @mvl_string_new(ptr @str_lit.58, i64 23)
  %e6 = load ptr, ptr %e4, align 8
  %str_concat = call ptr @mvl_string_concat(ptr %str_new5, ptr %e6)
  %str_cptr = call ptr @mvl_string_ptr(ptr %str_concat)
  %println = call i32 (ptr, ...) @printf(ptr @printf_fmt.59, ptr %str_cptr)
  ret i32 0

match_merge9:                                     ; preds = %arm011
  %match_val25 = phi i64 [ %p15, %arm011 ]
  %port = alloca i64, align 8
  store i64 %match_val25, ptr %port, align 4
  %str_new26 = call ptr @mvl_string_new(ptr @str_lit.62, i64 26)
  %port27 = load i64, ptr %port, align 4
  %int_str_buf = alloca [32 x i8], align 1
  %snprintf_int = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %int_str_buf, i64 32, ptr @int_fmt.63, i64 %port27)
  %str_len = zext i32 %snprintf_int to i64
  %str_new28 = call ptr @mvl_string_new(ptr %int_str_buf, i64 %str_len)
  %str_concat29 = call ptr @mvl_string_concat(ptr %str_new26, ptr %str_new28)
  %str_cptr30 = call ptr @mvl_string_ptr(ptr %str_concat29)
  %println31 = call i32 (ptr, ...) @printf(ptr @printf_fmt.64, ptr %str_cptr30)
  %actor_state = alloca %DbPoolState, align 8
  %field_connections = getelementptr inbounds nuw %DbPoolState, ptr %actor_state, i32 0, i32 0
  store i64 10, ptr %field_connections, align 4
  %actor_handle = call ptr @mvl_actor_spawn(ptr @db_pool_dispatch, ptr %actor_state, i64 8)
  %db = alloca ptr, align 8
  store ptr %actor_handle, ptr %db, align 8
  %actor_state32 = alloca %RequestHandlerState, align 8
  %db33 = load ptr, ptr %db, align 8
  %field_db = getelementptr inbounds nuw %RequestHandlerState, ptr %actor_state32, i32 0, i32 0
  store ptr %db33, ptr %field_db, align 8
  %actor_handle34 = call ptr @mvl_actor_spawn(ptr @request_handler_dispatch, ptr %actor_state32, i64 8)
  %h1 = alloca ptr, align 8
  store ptr %actor_handle34, ptr %h1, align 8
  %actor_state35 = alloca %RequestHandlerState, align 8
  %db36 = load ptr, ptr %db, align 8
  %field_db37 = getelementptr inbounds nuw %RequestHandlerState, ptr %actor_state35, i32 0, i32 0
  store ptr %db36, ptr %field_db37, align 8
  %actor_handle38 = call ptr @mvl_actor_spawn(ptr @request_handler_dispatch, ptr %actor_state35, i64 8)
  %h2 = alloca ptr, align 8
  store ptr %actor_handle38, ptr %h2, align 8
  %actor_state39 = alloca %RequestHandlerState, align 8
  %db40 = load ptr, ptr %db, align 8
  %field_db41 = getelementptr inbounds nuw %RequestHandlerState, ptr %actor_state39, i32 0, i32 0
  store ptr %db40, ptr %field_db41, align 8
  %actor_handle42 = call ptr @mvl_actor_spawn(ptr @request_handler_dispatch, ptr %actor_state39, i64 8)
  %h3 = alloca ptr, align 8
  store ptr %actor_handle42, ptr %h3, align 8
  %actor_state43 = alloca %TestClientState, align 8
  %port44 = load i64, ptr %port, align 4
  %field_port = getelementptr inbounds nuw %TestClientState, ptr %actor_state43, i32 0, i32 0
  store i64 %port44, ptr %field_port, align 4
  %actor_handle45 = call ptr @mvl_actor_spawn(ptr @test_client_dispatch, ptr %actor_state43, i64 8)
  %client = alloca ptr, align 8
  store ptr %actor_handle45, ptr %client, align 8
  %client46 = load ptr, ptr %client, align 8
  %actor_args = alloca [2 x i64], align 8
  %str_new47 = call ptr @mvl_string_new(ptr @str_lit.65, i64 3)
  %arg_i64 = ptrtoint ptr %str_new47 to i64
  %arg_ptr_0 = getelementptr inbounds i64, ptr %actor_args, i64 0
  store i64 %arg_i64, ptr %arg_ptr_0, align 4
  %str_new48 = call ptr @mvl_string_new(ptr @str_lit.66, i64 6)
  %arg_i6449 = ptrtoint ptr %str_new48 to i64
  %arg_ptr_1 = getelementptr inbounds i64, ptr %actor_args, i64 1
  store i64 %arg_i6449, ptr %arg_ptr_1, align 4
  call void @mvl_actor_send(ptr %client46, i64 0, i64 2, ptr %actor_args)
  %client50 = load ptr, ptr %client, align 8
  %actor_args51 = alloca [2 x i64], align 8
  %str_new52 = call ptr @mvl_string_new(ptr @str_lit.67, i64 3)
  %arg_i6453 = ptrtoint ptr %str_new52 to i64
  %arg_ptr_054 = getelementptr inbounds i64, ptr %actor_args51, i64 0
  store i64 %arg_i6453, ptr %arg_ptr_054, align 4
  %str_new55 = call ptr @mvl_string_new(ptr @str_lit.68, i64 7)
  %arg_i6456 = ptrtoint ptr %str_new55 to i64
  %arg_ptr_157 = getelementptr inbounds i64, ptr %actor_args51, i64 1
  store i64 %arg_i6456, ptr %arg_ptr_157, align 4
  call void @mvl_actor_send(ptr %client50, i64 0, i64 2, ptr %actor_args51)
  %client58 = load ptr, ptr %client, align 8
  %actor_args59 = alloca [2 x i64], align 8
  %str_new60 = call ptr @mvl_string_new(ptr @str_lit.69, i64 6)
  %arg_i6461 = ptrtoint ptr %str_new60 to i64
  %arg_ptr_062 = getelementptr inbounds i64, ptr %actor_args59, i64 0
  store i64 %arg_i6461, ptr %arg_ptr_062, align 4
  %str_new63 = call ptr @mvl_string_new(ptr @str_lit.70, i64 6)
  %arg_i6464 = ptrtoint ptr %str_new63 to i64
  %arg_ptr_165 = getelementptr inbounds i64, ptr %actor_args59, i64 1
  store i64 %arg_i6464, ptr %arg_ptr_165, align 4
  call void @mvl_actor_send(ptr %client58, i64 0, i64 2, ptr %actor_args59)
  %listener66 = load ptr, ptr %listener, align 8
  %h167 = load ptr, ptr %h1, align 8
  call void @accept_request(ptr %listener66, i64 1, ptr %h167)
  %listener68 = load ptr, ptr %listener, align 8
  %h269 = load ptr, ptr %h2, align 8
  call void @accept_request(ptr %listener68, i64 2, ptr %h269)
  %listener70 = load ptr, ptr %listener, align 8
  %h371 = load ptr, ptr %h3, align 8
  call void @accept_request(ptr %listener70, i64 3, ptr %h371)
  %listener72 = load ptr, ptr %listener, align 8
  call void @_mvl_net_tcp_close_listener(ptr %listener72)
  %println73 = call i32 (ptr, ...) @printf(ptr @println_fmt)
  ret i32 0

match_default10:                                  ; preds = %match_merge
  unreachable

arm011:                                           ; preds = %match_merge
  %payload_ptr13 = extractvalue { i8, ptr } %port_wrapped, 1
  %p = load i64, ptr %payload_ptr13, align 4
  %p14 = alloca i64, align 8
  store i64 %p, ptr %p14, align 4
  %p15 = load i64, ptr %p14, align 4
  br label %match_merge9

arm112:                                           ; preds = %match_merge
  %payload_ptr16 = extractvalue { i8, ptr } %port_wrapped, 1
  %e17 = load ptr, ptr %payload_ptr16, align 8
  %e18 = alloca ptr, align 8
  store ptr %e17, ptr %e18, align 8
  %listener19 = load ptr, ptr %listener, align 8
  call void @_mvl_net_tcp_close_listener(ptr %listener19)
  %str_new20 = call ptr @mvl_string_new(ptr @str_lit.60, i64 20)
  %e21 = load ptr, ptr %e18, align 8
  %str_concat22 = call ptr @mvl_string_concat(ptr %str_new20, ptr %e21)
  %str_cptr23 = call ptr @mvl_string_ptr(ptr %str_concat22)
  %println24 = call i32 (ptr, ...) @printf(ptr @printf_fmt.61, ptr %str_cptr23)
  ret i32 0
}

declare ptr @mvl_actor_spawn(ptr, ptr, i64)

declare void @mvl_actor_send(ptr, i64, i64, ptr)

declare void @mvl_actor_drop(ptr)

define void @test_client_send(ptr %self, ptr %method, ptr %path) {
entry:
  %port = getelementptr inbounds nuw %TestClientState, ptr %self, i32 0, i32 0
  %method1 = alloca ptr, align 8
  store ptr %method, ptr %method1, align 8
  %path2 = alloca ptr, align 8
  store ptr %path, ptr %path2, align 8
  %str_new = call ptr @mvl_string_new(ptr @str_lit, i64 9)
  %port3 = load i64, ptr %port, align 4
  %net_c_call = call { i8, ptr } @_mvl_net_tcp_connect(ptr %str_new, i64 %port3)
  %c_disc = extractvalue { i8, ptr } %net_c_call, 0
  %c_direct = extractvalue { i8, ptr } %net_c_call, 1
  %c_slot = alloca ptr, align 8
  store ptr %c_direct, ptr %c_slot, align 8
  %c_wrapped = alloca { i8, ptr }, align 8
  %c_disc_ptr = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped, i32 0, i32 0
  store i8 %c_disc, ptr %c_disc_ptr, align 1
  %c_payload_ptr = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped, i32 0, i32 1
  store ptr %c_slot, ptr %c_payload_ptr, align 8
  %c_result = load { i8, ptr }, ptr %c_wrapped, align 8
  %disc = extractvalue { i8, ptr } %c_result, 0
  switch i8 %disc, label %match_default [
    i8 0, label %arm0
    i8 1, label %arm1
  ]

match_merge:                                      ; preds = %arm1, %match_merge21
  ret void

match_default:                                    ; preds = %entry
  unreachable

arm0:                                             ; preds = %entry
  %payload_ptr = extractvalue { i8, ptr } %c_result, 1
  %s = load ptr, ptr %payload_ptr, align 8
  %s4 = alloca ptr, align 8
  store ptr %s, ptr %s4, align 8
  %method5 = load ptr, ptr %method1, align 8
  %str_new6 = call ptr @mvl_string_new(ptr @str_lit.1, i64 1)
  %str_concat = call ptr @mvl_string_concat(ptr %method5, ptr %str_new6)
  %path7 = load ptr, ptr %path2, align 8
  %str_concat8 = call ptr @mvl_string_concat(ptr %str_concat, ptr %path7)
  %str_new9 = call ptr @mvl_string_new(ptr @str_lit.2, i64 30)
  %str_concat10 = call ptr @mvl_string_concat(ptr %str_concat8, ptr %str_new9)
  %req_line = alloca ptr, align 8
  store ptr %str_concat10, ptr %req_line, align 8
  %s11 = load ptr, ptr %s4, align 8
  %req_line12 = load ptr, ptr %req_line, align 8
  %io_c_call = call { i8, ptr } @_mvl_net_tcp_write(ptr %s11, ptr %req_line12)
  %c_disc13 = extractvalue { i8, ptr } %io_c_call, 0
  %c_direct14 = extractvalue { i8, ptr } %io_c_call, 1
  %c_slot15 = alloca ptr, align 8
  store ptr %c_direct14, ptr %c_slot15, align 8
  %c_wrapped16 = alloca { i8, ptr }, align 8
  %c_disc_ptr17 = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped16, i32 0, i32 0
  store i8 %c_disc13, ptr %c_disc_ptr17, align 1
  %c_payload_ptr18 = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped16, i32 0, i32 1
  store ptr %c_slot15, ptr %c_payload_ptr18, align 8
  %c_result19 = load { i8, ptr }, ptr %c_wrapped16, align 8
  %disc20 = extractvalue { i8, ptr } %c_result19, 0
  switch i8 %disc20, label %match_default22 [
    i8 0, label %arm023
    i8 1, label %arm124
  ]

arm1:                                             ; preds = %entry
  %payload_ptr32 = extractvalue { i8, ptr } %c_result, 1
  %e33 = load ptr, ptr %payload_ptr32, align 8
  %e34 = alloca ptr, align 8
  store ptr %e33, ptr %e34, align 8
  %str_new35 = call ptr @mvl_string_new(ptr @str_lit.4, i64 27)
  %e36 = load ptr, ptr %e34, align 8
  %str_concat37 = call ptr @mvl_string_concat(ptr %str_new35, ptr %e36)
  %str_cptr38 = call ptr @mvl_string_ptr(ptr %str_concat37)
  %println39 = call i32 (ptr, ...) @printf(ptr @printf_fmt.5, ptr %str_cptr38)
  br label %match_merge

match_merge21:                                    ; preds = %arm124, %arm023
  %match_val = phi i8 [ 0, %arm023 ], [ 0, %arm124 ]
  br label %match_merge

match_default22:                                  ; preds = %arm0
  unreachable

arm023:                                           ; preds = %arm0
  %s25 = load ptr, ptr %s4, align 8
  call void @_mvl_net_tcp_close_stream(ptr %s25)
  br label %match_merge21

arm124:                                           ; preds = %arm0
  %payload_ptr26 = extractvalue { i8, ptr } %c_result19, 1
  %e = load ptr, ptr %payload_ptr26, align 8
  %e27 = alloca ptr, align 8
  store ptr %e, ptr %e27, align 8
  %str_new28 = call ptr @mvl_string_new(ptr @str_lit.3, i64 25)
  %e29 = load ptr, ptr %e27, align 8
  %str_concat30 = call ptr @mvl_string_concat(ptr %str_new28, ptr %e29)
  %str_cptr = call ptr @mvl_string_ptr(ptr %str_concat30)
  %println = call i32 (ptr, ...) @printf(ptr @printf_fmt, ptr %str_cptr)
  %s31 = load ptr, ptr %s4, align 8
  call void @_mvl_net_tcp_close_stream(ptr %s31)
  br label %match_merge21
}

declare ptr @mvl_string_new(ptr, i64)

declare { i8, ptr } @_mvl_net_tcp_connect(ptr, i64)

declare ptr @mvl_string_concat(ptr, ptr)

declare { i8, ptr } @_mvl_net_tcp_write(ptr, ptr)

declare void @_mvl_net_tcp_close_stream(ptr)

declare ptr @mvl_string_ptr(ptr)

declare i32 @printf(ptr, ...)

define void @test_client_dispatch(ptr %0, i64 %1, ptr %2) {
entry:
  switch i64 %1, label %default [
    i64 0, label %behavior_0
  ]

default:                                          ; preds = %entry
  ret void

behavior_0:                                       ; preds = %entry
  %arg_0 = getelementptr inbounds i64, ptr %2, i64 0
  %raw_0 = load i64, ptr %arg_0, align 4
  %ptr_0 = inttoptr i64 %raw_0 to ptr
  %arg_1 = getelementptr inbounds i64, ptr %2, i64 1
  %raw_1 = load i64, ptr %arg_1, align 4
  %ptr_1 = inttoptr i64 %raw_1 to ptr
  call void @test_client_send(ptr %0, ptr %ptr_0, ptr %ptr_1)
  ret void
}

define void @db_pool_query(ptr %self, ptr %sql, i64 %req_id, ptr %stream, ptr %caller) {
entry:
  %connections = getelementptr inbounds nuw %DbPoolState, ptr %self, i32 0, i32 0
  %sql1 = alloca ptr, align 8
  store ptr %sql, ptr %sql1, align 8
  %req_id2 = alloca i64, align 8
  store i64 %req_id, ptr %req_id2, align 4
  %stream3 = alloca ptr, align 8
  store ptr %stream, ptr %stream3, align 8
  %caller4 = alloca ptr, align 8
  store ptr %caller, ptr %caller4, align 8
  %str_new = call ptr @mvl_string_new(ptr @str_lit.6, i64 15)
  %sql5 = load ptr, ptr %sql1, align 8
  %str_concat = call ptr @mvl_string_concat(ptr %str_new, ptr %sql5)
  %str_new6 = call ptr @mvl_string_new(ptr @str_lit.7, i64 11)
  %str_concat7 = call ptr @mvl_string_concat(ptr %str_concat, ptr %str_new6)
  %req_id8 = load i64, ptr %req_id2, align 4
  %int_str_buf = alloca [32 x i8], align 1
  %snprintf_int = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %int_str_buf, i64 32, ptr @int_fmt, i64 %req_id8)
  %str_len = zext i32 %snprintf_int to i64
  %str_new9 = call ptr @mvl_string_new(ptr %int_str_buf, i64 %str_len)
  %str_concat10 = call ptr @mvl_string_concat(ptr %str_concat7, ptr %str_new9)
  %str_cptr = call ptr @mvl_string_ptr(ptr %str_concat10)
  %println = call i32 (ptr, ...) @printf(ptr @printf_fmt.8, ptr %str_cptr)
  %struct_tmp = alloca %QueryResult, align 8
  %str_new11 = call ptr @mvl_string_new(ptr @str_lit.9, i64 28)
  %f0_ptr = getelementptr inbounds nuw %QueryResult, ptr %struct_tmp, i32 0, i32 0
  store ptr %str_new11, ptr %f0_ptr, align 8
  %struct_val = load %QueryResult, ptr %struct_tmp, align 8
  %result = alloca %QueryResult, align 8
  store %QueryResult %struct_val, ptr %result, align 8
  %caller12 = load ptr, ptr %caller4, align 8
  %actor_args = alloca [3 x i64], align 8
  %result13 = load %QueryResult, ptr %result, align 8
  %arg_box = call ptr @mvl_box_new(i64 8)
  store %QueryResult %result13, ptr %arg_box, align 8
  %arg_i64 = ptrtoint ptr %arg_box to i64
  %arg_ptr_0 = getelementptr inbounds i64, ptr %actor_args, i64 0
  store i64 %arg_i64, ptr %arg_ptr_0, align 4
  %req_id14 = load i64, ptr %req_id2, align 4
  %arg_ptr_1 = getelementptr inbounds i64, ptr %actor_args, i64 1
  store i64 %req_id14, ptr %arg_ptr_1, align 4
  %stream15 = load ptr, ptr %stream3, align 8
  %arg_i6416 = ptrtoint ptr %stream15 to i64
  %arg_ptr_2 = getelementptr inbounds i64, ptr %actor_args, i64 2
  store i64 %arg_i6416, ptr %arg_ptr_2, align 4
  call void @mvl_actor_send(ptr %caller12, i64 1, i64 3, ptr %actor_args)
  ret void
}

declare i32 @snprintf(ptr, i64, ptr, ...)

declare ptr @mvl_box_new(i64)

define void @db_pool_dispatch(ptr %0, i64 %1, ptr %2) {
entry:
  switch i64 %1, label %default [
    i64 0, label %behavior_0
  ]

default:                                          ; preds = %entry
  ret void

behavior_0:                                       ; preds = %entry
  %arg_0 = getelementptr inbounds i64, ptr %2, i64 0
  %raw_0 = load i64, ptr %arg_0, align 4
  %ptr_0 = inttoptr i64 %raw_0 to ptr
  %arg_1 = getelementptr inbounds i64, ptr %2, i64 1
  %raw_1 = load i64, ptr %arg_1, align 4
  %arg_2 = getelementptr inbounds i64, ptr %2, i64 2
  %raw_2 = load i64, ptr %arg_2, align 4
  %ptr_2 = inttoptr i64 %raw_2 to ptr
  %arg_3 = getelementptr inbounds i64, ptr %2, i64 3
  %raw_3 = load i64, ptr %arg_3, align 4
  %ptr_3 = inttoptr i64 %raw_3 to ptr
  call void @db_pool_query(ptr %0, ptr %ptr_0, i64 %raw_1, ptr %ptr_2, ptr %ptr_3)
  ret void
}

define void @request_handler_handle(ptr %self, ptr %stream, %Request %req) {
entry:
  %db = getelementptr inbounds nuw %RequestHandlerState, ptr %self, i32 0, i32 0
  %stream1 = alloca ptr, align 8
  store ptr %stream, ptr %stream1, align 8
  %req2 = alloca %Request, align 8
  store %Request %req, ptr %req2, align 4
  %str_new = call ptr @mvl_string_new(ptr @str_lit.10, i64 17)
  %req3 = load %Request, ptr %req2, align 4
  %route = extractvalue %Request %req3, 2
  %call = call ptr @route_name(i8 %route)
  %str_concat = call ptr @mvl_string_concat(ptr %str_new, ptr %call)
  %str_new4 = call ptr @mvl_string_new(ptr @str_lit.11, i64 7)
  %str_concat5 = call ptr @mvl_string_concat(ptr %str_concat, ptr %str_new4)
  %req6 = load %Request, ptr %req2, align 4
  %id = extractvalue %Request %req6, 0
  %int_str_buf = alloca [32 x i8], align 1
  %snprintf_int = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %int_str_buf, i64 32, ptr @int_fmt.12, i64 %id)
  %str_len = zext i32 %snprintf_int to i64
  %str_new7 = call ptr @mvl_string_new(ptr %int_str_buf, i64 %str_len)
  %str_concat8 = call ptr @mvl_string_concat(ptr %str_concat5, ptr %str_new7)
  %str_new9 = call ptr @mvl_string_new(ptr @str_lit.13, i64 1)
  %str_concat10 = call ptr @mvl_string_concat(ptr %str_concat8, ptr %str_new9)
  %str_cptr = call ptr @mvl_string_ptr(ptr %str_concat10)
  %println = call i32 (ptr, ...) @printf(ptr @printf_fmt.14, ptr %str_cptr)
  %req11 = load %Request, ptr %req2, align 4
  %route12 = extractvalue %Request %req11, 2
  switch i8 %route12, label %match_default [
    i8 0, label %arm0
    i8 1, label %arm1
    i8 2, label %arm2
  ]

match_merge:                                      ; preds = %arm2, %arm1, %arm0
  ret void

match_default:                                    ; preds = %entry
  unreachable

arm0:                                             ; preds = %entry
  %db13 = load ptr, ptr %db, align 8
  br label %match_merge

arm1:                                             ; preds = %entry
  %str_new14 = call ptr @mvl_string_new(ptr @str_lit.15, i64 22)
  %req15 = load %Request, ptr %req2, align 4
  %id16 = extractvalue %Request %req15, 0
  %int_str_buf17 = alloca [32 x i8], align 1
  %snprintf_int18 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %int_str_buf17, i64 32, ptr @int_fmt.16, i64 %id16)
  %str_len19 = zext i32 %snprintf_int18 to i64
  %str_new20 = call ptr @mvl_string_new(ptr %int_str_buf17, i64 %str_len19)
  %str_concat21 = call ptr @mvl_string_concat(ptr %str_new14, ptr %str_new20)
  %str_new22 = call ptr @mvl_string_new(ptr @str_lit.17, i64 8)
  %str_concat23 = call ptr @mvl_string_concat(ptr %str_concat21, ptr %str_new22)
  %label = alloca ptr, align 8
  store ptr %str_concat23, ptr %label, align 8
  %stream24 = load ptr, ptr %stream1, align 8
  %str_new25 = call ptr @mvl_string_new(ptr @str_lit.18, i64 2)
  %call26 = call ptr @http_ok(ptr %str_new25)
  %label27 = load ptr, ptr %label, align 8
  call void @write_and_close(ptr %stream24, ptr %call26, ptr %label27)
  br label %match_merge

arm2:                                             ; preds = %entry
  %str_new28 = call ptr @mvl_string_new(ptr @str_lit.19, i64 29)
  %req29 = load %Request, ptr %req2, align 4
  %id30 = extractvalue %Request %req29, 0
  %int_str_buf31 = alloca [32 x i8], align 1
  %snprintf_int32 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %int_str_buf31, i64 32, ptr @int_fmt.20, i64 %id30)
  %str_len33 = zext i32 %snprintf_int32 to i64
  %str_new34 = call ptr @mvl_string_new(ptr %int_str_buf31, i64 %str_len33)
  %str_concat35 = call ptr @mvl_string_concat(ptr %str_new28, ptr %str_new34)
  %str_new36 = call ptr @mvl_string_new(ptr @str_lit.21, i64 1)
  %str_concat37 = call ptr @mvl_string_concat(ptr %str_concat35, ptr %str_new36)
  %label38 = alloca ptr, align 8
  store ptr %str_concat37, ptr %label38, align 8
  %stream39 = load ptr, ptr %stream1, align 8
  %call40 = call ptr @http_not_found()
  %label41 = load ptr, ptr %label38, align 8
  call void @write_and_close(ptr %stream39, ptr %call40, ptr %label41)
  br label %match_merge
}

define void @request_handler_query_done(ptr %self, %QueryResult %result, i64 %req_id, ptr %stream) {
entry:
  %db = getelementptr inbounds nuw %RequestHandlerState, ptr %self, i32 0, i32 0
  %result1 = alloca %QueryResult, align 8
  store %QueryResult %result, ptr %result1, align 8
  %req_id2 = alloca i64, align 8
  store i64 %req_id, ptr %req_id2, align 4
  %stream3 = alloca ptr, align 8
  store ptr %stream, ptr %stream3, align 8
  %str_new = call ptr @mvl_string_new(ptr @str_lit.22, i64 22)
  %req_id4 = load i64, ptr %req_id2, align 4
  %int_str_buf = alloca [32 x i8], align 1
  %snprintf_int = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %int_str_buf, i64 32, ptr @int_fmt.23, i64 %req_id4)
  %str_len = zext i32 %snprintf_int to i64
  %str_new5 = call ptr @mvl_string_new(ptr %int_str_buf, i64 %str_len)
  %str_concat = call ptr @mvl_string_concat(ptr %str_new, ptr %str_new5)
  %str_new6 = call ptr @mvl_string_new(ptr @str_lit.24, i64 6)
  %str_concat7 = call ptr @mvl_string_concat(ptr %str_concat, ptr %str_new6)
  %result8 = load %QueryResult, ptr %result1, align 8
  %data = extractvalue %QueryResult %result8, 0
  %str_concat9 = call ptr @mvl_string_concat(ptr %str_concat7, ptr %data)
  %label = alloca ptr, align 8
  store ptr %str_concat9, ptr %label, align 8
  %stream10 = load ptr, ptr %stream3, align 8
  %result11 = load %QueryResult, ptr %result1, align 8
  %data12 = extractvalue %QueryResult %result11, 0
  %call = call ptr @http_ok(ptr %data12)
  %label13 = load ptr, ptr %label, align 8
  call void @write_and_close(ptr %stream10, ptr %call, ptr %label13)
  ret void
}

define void @request_handler_dispatch(ptr %0, i64 %1, ptr %2) {
entry:
  switch i64 %1, label %default [
    i64 0, label %behavior_0
    i64 1, label %behavior_1
  ]

default:                                          ; preds = %entry
  ret void

behavior_0:                                       ; preds = %entry
  %arg_0 = getelementptr inbounds i64, ptr %2, i64 0
  %raw_0 = load i64, ptr %arg_0, align 4
  %ptr_0 = inttoptr i64 %raw_0 to ptr
  %arg_1 = getelementptr inbounds i64, ptr %2, i64 1
  %raw_1 = load i64, ptr %arg_1, align 4
  %arg_ptr_1 = inttoptr i64 %raw_1 to ptr
  %arg_val_1 = load %Request, ptr %arg_ptr_1, align 4
  call void @request_handler_handle(ptr %0, ptr %ptr_0, %Request %arg_val_1)
  ret void

behavior_1:                                       ; preds = %entry
  %arg_01 = getelementptr inbounds i64, ptr %2, i64 0
  %raw_02 = load i64, ptr %arg_01, align 4
  %arg_ptr_0 = inttoptr i64 %raw_02 to ptr
  %arg_val_0 = load %QueryResult, ptr %arg_ptr_0, align 8
  %arg_13 = getelementptr inbounds i64, ptr %2, i64 1
  %raw_14 = load i64, ptr %arg_13, align 4
  %arg_2 = getelementptr inbounds i64, ptr %2, i64 2
  %raw_2 = load i64, ptr %arg_2, align 4
  %ptr_2 = inttoptr i64 %raw_2 to ptr
  call void @request_handler_query_done(ptr %0, %QueryResult %arg_val_0, i64 %raw_14, ptr %ptr_2)
  ret void
}

declare ptr @mvl_array_new(i64, i64)

declare void @mvl_array_push(ptr, ptr)

; Function Attrs: nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none)
declare { i64, i1 } @llvm.sadd.with.overflow.i64(i64, i64) #0

; Function Attrs: cold noreturn nounwind memory(inaccessiblemem: write)
declare void @llvm.trap() #1

declare void @mvl_string_ptr_array_drop(ptr)

declare void @mvl_string_drop(ptr)

declare i64 @mvl_string_len(ptr)

declare { i8, ptr } @_mvl_net_tcp_accept(ptr)

declare { i8, ptr } @_mvl_net_tcp_read(ptr)

declare { i8, ptr } @_mvl_net_tcp_listen(ptr, i64)

declare { i8, ptr } @_mvl_net_tcp_listener_port(ptr)

declare void @_mvl_net_tcp_close_listener(ptr)

declare ptr @mvl_string_chars(ptr)

declare i64 @mvl_array_len(ptr)

declare ptr @mvl_array_get(ptr, i64)

attributes #0 = { nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none) }
attributes #1 = { cold noreturn nounwind memory(inaccessiblemem: write) }
