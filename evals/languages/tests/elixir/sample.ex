defmodule Conformance do
  def anchor_elixir(value), do: value + 41
  def caller_elixir(), do: anchor_elixir(1)
end
