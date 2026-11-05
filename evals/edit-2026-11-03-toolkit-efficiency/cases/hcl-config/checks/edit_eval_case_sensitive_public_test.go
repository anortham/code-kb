package json_test

import (
	"testing"

	"github.com/hashicorp/hcl/v2"
	hcljson "github.com/hashicorp/hcl/v2/json"
	"github.com/zclconf/go-cty/cty"
)

func TestEditEvalPublicJSONRegion(t *testing.T) {
	object, diags := hcljson.ParseExpression([]byte(`{"Region":"north"}`), "test.json", hcl.InitialPos)
	if diags.HasErrors() {
		t.Fatal(diags.Error())
	}
	value, diags := object.Value(nil)
	if diags.HasErrors() {
		t.Fatal(diags.Error())
	}
	if !value.Type().HasAttribute("Region") || value.Type().HasAttribute("region") {
		t.Fatalf("JSON object keys lost their spelling: %s", value.Type().FriendlyName())
	}

	ctx := &hcl.EvalContext{Variables: map[string]cty.Value{
		"local": cty.ObjectVal(map[string]cty.Value{
			"settings": cty.ObjectVal(map[string]cty.Value{"Region": cty.StringVal("north")}),
			"tags":     cty.MapVal(map[string]cty.Value{"Release": cty.StringVal("stable")}),
		}),
	}}
	for _, test := range []struct {
		expression string
		want       string
	}{
		{`"${local.settings.Region}"`, "north"},
		{`"${local.tags.Release}"`, "stable"},
	} {
		expr, diags := hcljson.ParseExpression([]byte(test.expression), "test.json", hcl.InitialPos)
		if diags.HasErrors() {
			t.Fatalf("parse %q: %s", test.expression, diags.Error())
		}
		got, diags := expr.Value(ctx)
		if diags.HasErrors() {
			t.Fatalf("evaluate %q: %s", test.expression, diags.Error())
		}
		if got.AsString() != test.want {
			t.Errorf("%q = %q, want %q", test.expression, got.AsString(), test.want)
		}
	}
}
