; ModuleID = 'net_basic'
source_filename = "net_basic"
target triple = "arm64-apple-darwin24.6.0"

%ConnectorState = type { i64 }

@str_lit = private unnamed_addr constant [10 x i8] c"127.0.0.1\00", align 1
@str_lit.1 = private unnamed_addr constant [1 x i8] zeroinitializer, align 1
@str_lit.2 = private unnamed_addr constant [10 x i8] c"127.0.0.1\00", align 1
@str_lit.3 = private unnamed_addr constant [15 x i8] c"listen error: \00", align 1
@printf_fmt = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.4 = private unnamed_addr constant [13 x i8] c"port error: \00", align 1
@printf_fmt.5 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.6 = private unnamed_addr constant [15 x i8] c"accept error: \00", align 1
@printf_fmt.7 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@printf_fmt.8 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1
@str_lit.9 = private unnamed_addr constant [13 x i8] c"read error: \00", align 1
@printf_fmt.10 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1

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
  %str_new = call ptr @mvl_string_new(ptr @str_lit.1, i64 0)
  %drop_xs = load ptr, ptr %xs1, align 8
  call void @mvl_string_ptr_array_drop(ptr %drop_xs)
  %drop_sep = load ptr, ptr %sep2, align 8
  call void @mvl_string_drop(ptr %drop_sep)
  ret ptr %str_new
}

define { i8, ptr } @tcp_listen(ptr %0, i64 %1) {
entry:
  ret { i8, ptr } { i8 1, ptr null }
}

define { i8, ptr } @tcp_connect(ptr %0, i64 %1) {
entry:
  ret { i8, ptr } { i8 1, ptr null }
}

define { i8, ptr } @tcp_accept(ptr %0) {
entry:
  ret { i8, ptr } { i8 1, ptr null }
}

define { i8, ptr } @tcp_read(ptr %0) {
entry:
  ret { i8, ptr } { i8 1, ptr null }
}

define { i8, ptr } @tcp_write(ptr %0, ptr %1) {
entry:
  ret { i8, ptr } { i8 1, ptr null }
}

define { i8, ptr } @tcp_listener_port(ptr %0) {
entry:
  ret { i8, ptr } { i8 1, ptr null }
}

define void @tcp_close_listener(ptr %0) {
entry:
  ret void
}

define void @tcp_close_stream(ptr %0) {
entry:
  ret void
}

define i32 @main() {
entry:
  %str_new = call ptr @mvl_string_new(ptr @str_lit.2, i64 9)
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
  %str_new5 = call ptr @mvl_string_new(ptr @str_lit.3, i64 14)
  %e6 = load ptr, ptr %e4, align 8
  %str_concat = call ptr @mvl_string_concat(ptr %str_new5, ptr %e6)
  %str_cptr = call ptr @mvl_string_ptr(ptr %str_concat)
  %println = call i32 (ptr, ...) @printf(ptr @printf_fmt, ptr %str_cptr)
  ret i32 0

match_merge9:                                     ; preds = %arm011
  %match_val24 = phi i64 [ %p15, %arm011 ]
  %port = alloca i64, align 8
  store i64 %match_val24, ptr %port, align 4
  %actor_state = alloca %ConnectorState, align 8
  %port25 = load i64, ptr %port, align 4
  %field_port = getelementptr inbounds nuw %ConnectorState, ptr %actor_state, i32 0, i32 0
  store i64 %port25, ptr %field_port, align 4
  %actor_handle = call ptr @mvl_actor_spawn(ptr @connector_dispatch, ptr %actor_state, i64 8)
  %c = alloca ptr, align 8
  store ptr %actor_handle, ptr %c, align 8
  %c26 = load ptr, ptr %c, align 8
  call void @mvl_actor_send(ptr %c26, i64 0, i64 0, ptr null)
  %listener27 = load ptr, ptr %listener, align 8
  %io_c_call = call { i8, ptr } @_mvl_net_tcp_accept(ptr %listener27)
  %c_disc28 = extractvalue { i8, ptr } %io_c_call, 0
  %c_direct29 = extractvalue { i8, ptr } %io_c_call, 1
  %c_slot30 = alloca ptr, align 8
  store ptr %c_direct29, ptr %c_slot30, align 8
  %c_wrapped31 = alloca { i8, ptr }, align 8
  %c_disc_ptr32 = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped31, i32 0, i32 0
  store i8 %c_disc28, ptr %c_disc_ptr32, align 1
  %c_payload_ptr33 = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped31, i32 0, i32 1
  store ptr %c_slot30, ptr %c_payload_ptr33, align 8
  %c_result34 = load { i8, ptr }, ptr %c_wrapped31, align 8
  %disc35 = extractvalue { i8, ptr } %c_result34, 0
  switch i8 %disc35, label %match_default37 [
    i8 0, label %arm038
    i8 1, label %arm139
  ]

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
  %str_new19 = call ptr @mvl_string_new(ptr @str_lit.4, i64 12)
  %e20 = load ptr, ptr %e18, align 8
  %str_concat21 = call ptr @mvl_string_concat(ptr %str_new19, ptr %e20)
  %str_cptr22 = call ptr @mvl_string_ptr(ptr %str_concat21)
  %println23 = call i32 (ptr, ...) @printf(ptr @printf_fmt.5, ptr %str_cptr22)
  ret i32 0

match_merge36:                                    ; preds = %arm038
  %match_val51 = phi ptr [ %s42, %arm038 ]
  %server_stream = alloca ptr, align 8
  store ptr %match_val51, ptr %server_stream, align 8
  %server_stream52 = load ptr, ptr %server_stream, align 8
  %io_c_call53 = call { i8, ptr } @_mvl_net_tcp_read(ptr %server_stream52)
  %c_disc54 = extractvalue { i8, ptr } %io_c_call53, 0
  %c_direct55 = extractvalue { i8, ptr } %io_c_call53, 1
  %c_slot56 = alloca ptr, align 8
  store ptr %c_direct55, ptr %c_slot56, align 8
  %c_wrapped57 = alloca { i8, ptr }, align 8
  %c_disc_ptr58 = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped57, i32 0, i32 0
  store i8 %c_disc54, ptr %c_disc_ptr58, align 1
  %c_payload_ptr59 = getelementptr inbounds nuw { i8, ptr }, ptr %c_wrapped57, i32 0, i32 1
  store ptr %c_slot56, ptr %c_payload_ptr59, align 8
  %c_result60 = load { i8, ptr }, ptr %c_wrapped57, align 8
  %read_result = alloca { i8, ptr }, align 8
  store { i8, ptr } %c_result60, ptr %read_result, align 8
  %server_stream61 = load ptr, ptr %server_stream, align 8
  call void @_mvl_net_tcp_close_stream(ptr %server_stream61)
  %listener62 = load ptr, ptr %listener, align 8
  call void @_mvl_net_tcp_close_listener(ptr %listener62)
  %read_result63 = load { i8, ptr }, ptr %read_result, align 8
  %disc64 = extractvalue { i8, ptr } %read_result63, 0
  switch i8 %disc64, label %match_default66 [
    i8 0, label %arm067
    i8 1, label %arm168
  ]

match_default37:                                  ; preds = %match_merge9
  unreachable

arm038:                                           ; preds = %match_merge9
  %payload_ptr40 = extractvalue { i8, ptr } %c_result34, 1
  %s = load ptr, ptr %payload_ptr40, align 8
  %s41 = alloca ptr, align 8
  store ptr %s, ptr %s41, align 8
  %s42 = load ptr, ptr %s41, align 8
  br label %match_merge36

arm139:                                           ; preds = %match_merge9
  %payload_ptr43 = extractvalue { i8, ptr } %c_result34, 1
  %e44 = load ptr, ptr %payload_ptr43, align 8
  %e45 = alloca ptr, align 8
  store ptr %e44, ptr %e45, align 8
  %str_new46 = call ptr @mvl_string_new(ptr @str_lit.6, i64 14)
  %e47 = load ptr, ptr %e45, align 8
  %str_concat48 = call ptr @mvl_string_concat(ptr %str_new46, ptr %e47)
  %str_cptr49 = call ptr @mvl_string_ptr(ptr %str_concat48)
  %println50 = call i32 (ptr, ...) @printf(ptr @printf_fmt.7, ptr %str_cptr49)
  ret i32 0

match_merge65:                                    ; preds = %arm168, %arm067
  ret i32 0

match_default66:                                  ; preds = %match_merge36
  unreachable

arm067:                                           ; preds = %match_merge36
  %payload_ptr69 = extractvalue { i8, ptr } %read_result63, 1
  %msg = load ptr, ptr %payload_ptr69, align 8
  %msg70 = alloca ptr, align 8
  store ptr %msg, ptr %msg70, align 8
  %msg71 = load ptr, ptr %msg70, align 8
  %str_cptr72 = call ptr @mvl_string_ptr(ptr %msg71)
  %println73 = call i32 (ptr, ...) @printf(ptr @printf_fmt.8, ptr %str_cptr72)
  br label %match_merge65

arm168:                                           ; preds = %match_merge36
  %payload_ptr74 = extractvalue { i8, ptr } %read_result63, 1
  %e75 = load ptr, ptr %payload_ptr74, align 8
  %e76 = alloca ptr, align 8
  store ptr %e75, ptr %e76, align 8
  %str_new77 = call ptr @mvl_string_new(ptr @str_lit.9, i64 12)
  %e78 = load ptr, ptr %e76, align 8
  %str_concat79 = call ptr @mvl_string_concat(ptr %str_new77, ptr %e78)
  %str_cptr80 = call ptr @mvl_string_ptr(ptr %str_concat79)
  %println81 = call i32 (ptr, ...) @printf(ptr @printf_fmt.10, ptr %str_cptr80)
  br label %match_merge65
}

declare ptr @mvl_actor_spawn(ptr, ptr, i64)

declare void @mvl_actor_send(ptr, i64, i64, ptr)

declare void @mvl_actor_drop(ptr)

define void @connector_connect(ptr %self) {
entry:
  %port = getelementptr inbounds nuw %ConnectorState, ptr %self, i32 0, i32 0
  %str_new = call ptr @mvl_string_new(ptr @str_lit, i64 9)
  ret void
}

declare ptr @mvl_string_new(ptr, i64)

define void @connector_dispatch(ptr %0, i64 %1, ptr %2) {
entry:
  switch i64 %1, label %default [
    i64 0, label %behavior_0
  ]

default:                                          ; preds = %entry
  ret void

behavior_0:                                       ; preds = %entry
  call void @connector_connect(ptr %0)
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

declare { i8, ptr } @_mvl_net_tcp_listen(ptr, i64)

declare ptr @mvl_string_concat(ptr, ptr)

declare ptr @mvl_string_ptr(ptr)

declare i32 @printf(ptr, ...)

declare { i8, ptr } @_mvl_net_tcp_listener_port(ptr)

declare { i8, ptr } @_mvl_net_tcp_accept(ptr)

declare { i8, ptr } @_mvl_net_tcp_read(ptr)

declare void @_mvl_net_tcp_close_stream(ptr)

declare void @_mvl_net_tcp_close_listener(ptr)

declare ptr @mvl_string_chars(ptr)

declare i64 @mvl_array_len(ptr)

declare ptr @mvl_array_get(ptr, i64)

attributes #0 = { nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none) }
attributes #1 = { cold noreturn nounwind memory(inaccessiblemem: write) }
