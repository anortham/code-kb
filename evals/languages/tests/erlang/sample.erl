-module(conformance).
-export([anchor_erlang/1, caller_erlang/0]).
anchor_erlang(Value) -> Value + 41.
caller_erlang() -> anchor_erlang(1).
