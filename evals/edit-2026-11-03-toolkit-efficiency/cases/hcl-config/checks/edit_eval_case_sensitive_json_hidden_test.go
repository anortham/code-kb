package json_test

import (
	"strings"
	"testing"

	"github.com/hashicorp/hcl/v2"
	hcljson "github.com/hashicorp/hcl/v2/json"
	"github.com/zclconf/go-cty/cty"
)

func TestEditEvalCaseSensitiveJSONExpression(t *testing.T) {
	object, diags := hcljson.ParseExpression([]byte(`{"Region":"north"}`), "test.json")
	if diags.HasErrors() {
		t.Fatal(diags.Error())
	}
	value, diags := object.Value(nil)
	if diags.HasErrors() || !value.Type().HasAttribute("Region") || value.Type().HasAttribute("region") {
		t.Fatalf("JSON object keys must retain their spelling: %s %s", value.Type().FriendlyName(), diags.Error())
	}
	ctx := &hcl.EvalContext{Variables: map[string]cty.Value{
		"local": cty.ObjectVal(map[string]cty.Value{"settings": cty.ObjectVal(map[string]cty.Value{"Region": cty.StringVal("north")})}),
	}}
	exact, diags := hcljson.ParseExpression([]byte(`"${local.settings.Region}"`), "test.json")
	if diags.HasErrors() {
		t.Fatal(diags.Error())
	}
	got, diags := exact.Value(ctx)
	if diags.HasErrors() || got.AsString() != "north" {
		t.Fatalf("exact-case JSON traversal failed: %v %s", got, diags.Error())
	}
	wrong, diags := hcljson.ParseExpression([]byte(`"${local.settings.region}"`), "test.json")
	if diags.HasErrors() {
		t.Fatal(diags.Error())
	}
	_, diags = wrong.Value(ctx)
	if !diags.HasErrors() || !strings.Contains(diags.Error(), "Unsupported attribute") {
		t.Fatalf("case sensitivity must be preserved in JSON traversal; diagnostics: %s", diags.Error())
	}
}
