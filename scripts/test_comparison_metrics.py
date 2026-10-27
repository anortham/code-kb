import unittest

from comparison_metrics import grade_answer, summarize_trace


def symbol(name, file, line, owner=None):
    return {"name": name, "file": file, "line": line, "owner": owner, "reason": "fixture evidence"}


def test(name, file, command=None, command_status="unavailable"):
    return {
        "name": name,
        "file": file,
        "command": command,
        "command_status": command_status,
        "reason": "focused test",
    }


def answer(symbols=(), tests=(), literals=()):
    return {
        "symbols": list(symbols),
        "tests": list(tests),
        "literals": list(literals),
        "explanation": "Source chain needs review.",
    }


class SummarizeTraceTest(unittest.TestCase):
    def test_missing_usage_stays_unavailable_and_completed_items_are_counted_once(self):
        command = {"id": "cmd-1", "type": "command_execution", "command": "cargo test"}
        lookup = {
            "id": "mcp-1",
            "type": "mcp_tool_call",
            "server": "code-kb",
            "tool": "lookup_symbol",
            "arguments": {"query": "InvitationService"},
        }
        result = summarize_trace(
            [
                {"type": "item.started", "item": command},
                {"type": "item.started", "item": lookup},
                {"type": "item.completed", "item": lookup},
                {"type": "item.completed", "item": command},
                {"type": "item.completed", "item": command},
                {"type": "turn.completed"},
            ]
        )

        self.assertIsNone(result["input_tokens"])
        self.assertIsNone(result["output_tokens"])
        self.assertIsNone(result["cache_tokens"])
        self.assertEqual(result["tool_calls"], 2)
        self.assertEqual(result["code_kb_calls"], 1)
        self.assertEqual(result["commands"], ["cargo test"])
        self.assertEqual(len(result["mcp_calls"]), 1)
        self.assertEqual([action["id"] for action in result["actions"]], ["cmd-1", "mcp-1"])
        self.assertEqual([action["kind"] for action in result["actions"]], ["command", "mcp"])
        self.assertTrue(result["completed"])

    def test_legacy_total_usage_preserves_real_zero_cache_tokens(self):
        result = summarize_trace(
            [
                {
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 27,
                                "cached_input_tokens": 0,
                                "output_tokens": 6,
                            }
                        },
                    },
                },
                {"type": "event_msg", "payload": {"type": "task_complete"}},
            ]
        )

        self.assertEqual(result["input_tokens"], 27)
        self.assertEqual(result["output_tokens"], 6)
        self.assertEqual(result["cache_tokens"], 0)
        self.assertTrue(result["completed"])

    def test_jsonl_turn_usage_maps_input_output_and_cache_fields(self):
        result = summarize_trace(
            [
                {
                    "type": "turn.completed",
                    "usage": {"input_tokens": 24763, "cached_input_tokens": 24448, "output_tokens": 122},
                }
            ]
        )

        self.assertEqual(result["input_tokens"], 24763)
        self.assertEqual(result["output_tokens"], 122)
        self.assertEqual(result["cache_tokens"], 24448)
        self.assertTrue(result["completed"])

    def test_failed_turn_records_its_error_without_marking_the_trace_completed(self):
        result = summarize_trace([{"type": "turn.failed", "error": {"message": "network reset"}}])

        self.assertEqual(result["errors"], ["network reset"])
        self.assertFalse(result["completed"])


class GradeAnswerTest(unittest.TestCase):
    def test_valid_python_answer_can_leave_command_unavailable(self):
        oracle = {
            "symbols": [{"name": "register_account", "file": "python/app.py", "line": 10, "definition_line": 11}],
            "tests": ["python/test_app.py::test_register_account_route"],
        }

        result = grade_answer(
            answer(
                [symbol("register_account", "python/app.py", 11)],
                [test("test_register_account_route", "python/test_app.py")],
            ),
            oracle,
        )

        self.assertTrue(result["mechanical_pass"])
        self.assertTrue(result["review_required"])
        self.assertEqual(result["failures"], [])

    def test_qualified_test_names_match_the_expected_terminal_name(self):
        oracle = {"tests": ["python/test_app.py::test_register_account_route"]}

        for name in ("RegistrationTests.test_register_account_route", "RegistrationTests::test_register_account_route"):
            with self.subTest(name=name):
                result = grade_answer(
                    answer(tests=[test(name, "python/test_app.py")]),
                    oracle,
                )

                self.assertTrue(result["mechanical_pass"])

    def test_incorrect_extra_test_is_rejected_when_the_expected_test_is_present(self):
        oracle = {"tests": ["python/test_app.py::test_register_account_route"]}
        response = answer(
            tests=[
                test("test_register_account_route", "python/test_app.py"),
                test("test_other_route", "python/test_app.py"),
            ]
        )

        result = grade_answer(response, oracle)

        self.assertFalse(result["mechanical_pass"])
        self.assertTrue(any("unexpected test" in failure for failure in result["failures"]))

    def test_go_decoy_and_conflicting_method_owners_fail(self):
        oracle = {
            "symbols": [
                {"name": "Send", "file": "go/invitation.go", "line": 7},
                {"name": "ServeHTTP", "file": "go/invitation.go", "line": 19},
            ],
            "excluded_candidates": [
                {"name": "Send", "file": "go/invitation.go", "line": 13, "owner": "MailService"}
            ],
            "tests": ["go/invitation_test.go::TestInvitationRoute"],
        }
        response = answer(
            [
                symbol("Send", "go/invitation.go", 7, "InvitationService"),
                symbol("ServeHTTP", "go/invitation.go", 19, "InvitationHandler"),
                symbol("Send", "go/invitation.go", 13, "MailService"),
            ],
            [test("TestInvitationRoute", "go/invitation_test.go")],
        )

        result = grade_answer(response, oracle)

        self.assertFalse(result["mechanical_pass"])
        self.assertTrue(any("excluded" in failure.lower() for failure in result["failures"]))

    def test_conflicting_owners_for_one_required_symbol_fail(self):
        oracle = {"symbols": [{"name": "Send", "file": "go/invitation.go", "line": 7}]}
        response = answer(
            [
                symbol("Send", "go/invitation.go", 7, "InvitationService"),
                symbol("Send", "go/invitation.go", 7, "MailService"),
            ]
        )

        result = grade_answer(response, oracle)

        self.assertFalse(result["mechanical_pass"])
        self.assertTrue(any("conflicting owners" in failure for failure in result["failures"]))

    def test_wrong_symbol_line_and_wrong_test_path_fail(self):
        oracle = {
            "symbols": [{"name": "post", "file": "java/src/main/java/review/InvitationController.java", "line": 10}],
            "tests": ["java/src/test/java/review/InvitationControllerTest.java::post_sends_invitation"],
            "acceptance": ["The focused test uses JUnit 5, but no Java build file is present, so no runnable test command is evidenced."],
        }
        response = answer(
            [symbol("post", "java/src/main/java/review/InvitationController.java", 9)],
            [test("post_sends_invitation", "java/src/test/java/review/OtherTest.java")],
        )

        result = grade_answer(response, oracle)

        self.assertFalse(result["mechanical_pass"])
        self.assertTrue(any("post" in failure for failure in result["failures"]))
        self.assertTrue(any("test" in failure.lower() for failure in result["failures"]))

    def test_malformed_command_status_is_a_grade_failure(self):
        response = answer(tests=[{"name": "test_case", "file": "suite.py", "command": None, "command_status": [], "reason": ""}])

        result = grade_answer(response, {"tests": ["suite.py::test_case"]})

        self.assertFalse(result["mechanical_pass"])
        self.assertTrue(any("command_status" in failure for failure in result["failures"]))

    def test_rust_command_allows_only_fixture_manifest_relocation(self):
        oracle = {
            "symbols": [{"name": "send", "file": "rust/src/lib.rs", "line": 4}],
            "tests": ["rust/tests/invitation.rs::handler_sends_invitation"],
            "acceptance": [
                "Run `cargo test --manifest-path evals/review-2026-10-26/fixture/Cargo.toml handler_sends_invitation`."
            ],
        }
        correct = answer(
            [symbol("send", "rust/src/lib.rs", 4)],
            [
                test(
                    "handler_sends_invitation",
                    "rust/tests/invitation.rs",
                    "cargo test --manifest-path Cargo.toml handler_sends_invitation",
                    "evidenced",
                )
            ],
        )
        incorrect = answer(
            [symbol("send", "rust/src/lib.rs", 4)],
            [test("handler_sends_invitation", "rust/tests/invitation.rs", "cargo test handler_sends_invitation", "evidenced")],
        )
        joined_manifest = answer(
            [symbol("send", "rust/src/lib.rs", 4)],
            [
                test(
                    "handler_sends_invitation",
                    "rust/tests/invitation.rs",
                    "cargo test --manifest-path=Cargo.toml handler_sends_invitation",
                    "evidenced",
                )
            ],
        )

        self.assertTrue(grade_answer(correct, oracle)["mechanical_pass"])
        self.assertTrue(grade_answer(joined_manifest, oracle)["mechanical_pass"])
        self.assertFalse(grade_answer(incorrect, oracle)["mechanical_pass"])

    def test_declared_command_alternatives_accept_only_exact_token_forms(self):
        oracle = {
            "symbols": [{"name": "send", "file": "rust/src/lib.rs", "line": 4}],
            "tests": ["rust/tests/invitation.rs::handler_sends_invitation"],
            "acceptance": [
                "Run `cargo test --manifest-path evals/review-2026-10-26/fixture/Cargo.toml handler_sends_invitation`."
            ],
            "command_alternatives": [
                "cargo test -p review-cross-language-fixture --test invitation handler_sends_invitation",
                "cargo test -p review-cross-language-fixture --test invitation handler_sends_invitation -- --exact",
            ],
        }
        accepted = (
            "cargo test -p review-cross-language-fixture --test invitation handler_sends_invitation",
            "cargo test -p review-cross-language-fixture --test invitation handler_sends_invitation -- --exact",
            'cargo test -p review-cross-language-fixture --test invitation "handler_sends_invitation" -- --exact',
        )
        rejected = (
            "cargo test -p other-fixture --test invitation handler_sends_invitation",
            "cargo test -p review-cross-language-fixture --test other handler_sends_invitation",
            "cargo test -p review-cross-language-fixture --test invitation another_test -- --exact",
            "cargo test -p review-cross-language-fixture --test invitation handler_sends_invitation -- --ignored",
            "cargo test",
        )

        for command in accepted:
            with self.subTest(command=command):
                result = grade_answer(
                    answer(
                        [symbol("send", "rust/src/lib.rs", 4)],
                        [test("handler_sends_invitation", "rust/tests/invitation.rs", command, "evidenced")],
                    ),
                    oracle,
                )

                self.assertTrue(result["mechanical_pass"])
                self.assertTrue(result["review_required"])
                self.assertTrue(any("source review" in check for check in result["checks"]))

        for command in rejected:
            with self.subTest(command=command):
                result = grade_answer(
                    answer(
                        [symbol("send", "rust/src/lib.rs", 4)],
                        [test("handler_sends_invitation", "rust/tests/invitation.rs", command, "evidenced")],
                    ),
                    oracle,
                )

                self.assertFalse(result["mechanical_pass"])

        base_oracle = {
            "symbols": [{"name": "send", "file": "rust/src/lib.rs", "line": 4}],
            "tests": ["rust/tests/invitation.rs::handler_sends_invitation"],
            "acceptance": [
                "Run `cargo test --manifest-path evals/review-2026-10-26/fixture/Cargo.toml handler_sends_invitation`."
            ],
        }
        command = "cargo test -p review-cross-language-fixture --test invitation handler_sends_invitation"
        response = answer(
            [symbol("send", "rust/src/lib.rs", 4)],
            [test("handler_sends_invitation", "rust/tests/invitation.rs", command, "evidenced")],
        )

        alternative_cases = (
            (False, None),
            (True, None),
            (True, "cargo test -p review-cross-language-fixture --test invitation handler_sends_invitation"),
            (True, [None]),
            (True, ['"']),
            (True, []),
        )
        for present, alternatives in alternative_cases:
            with self.subTest(present=present, alternatives=alternatives):
                oracle = dict(base_oracle)
                if present:
                    oracle["command_alternatives"] = alternatives

                result = grade_answer(response, oracle)

                self.assertFalse(result["mechanical_pass"])
                self.assertTrue(any("focused command" in failure for failure in result["failures"]))

    def test_unavailable_command_is_required_when_oracle_says_no_build_file(self):
        oracle = {
            "symbols": [],
            "tests": ["typescript/invitation.test.ts::test_invitation_route_passes_display_name"],
            "acceptance": ["The case has no runnable package test command; say that the broader test command is unavailable."],
        }
        unavailable = answer(
            tests=[test("test_invitation_route_passes_display_name", "typescript/invitation.test.ts")]
        )
        invented = answer(
            tests=[
                test(
                    "test_invitation_route_passes_display_name",
                    "typescript/invitation.test.ts",
                    "npm test",
                    "evidenced",
                )
            ]
        )

        self.assertTrue(grade_answer(unavailable, oracle)["mechanical_pass"])
        self.assertFalse(grade_answer(invented, oracle)["mechanical_pass"])

    def test_java_and_csharp_oracles_also_require_unavailable_commands(self):
        cases = [
            {
                "symbols": [
                    {"name": "send", "file": "java/src/main/java/review/InvitationService.java", "line": 4},
                    {"name": "post", "file": "java/src/main/java/review/InvitationController.java", "line": 10},
                ],
                "tests": ["java/src/test/java/review/InvitationControllerTest.java::post_sends_invitation"],
                "acceptance": ["No Java build file is present, so no runnable test command is evidenced."],
            },
            {
                "symbols": [
                    {"name": "Send", "file": "csharp/InvitationService.cs", "line": 5},
                    {"name": "Invite", "file": "csharp/InvitationController.cs", "line": 4},
                ],
                "tests": ["csharp/InvitationControllerTests.cs::Invite_sends_invitation"],
                "acceptance": ["No C# project file is present, so no runnable test command is evidenced."],
            },
        ]

        for oracle in cases:
            with self.subTest(test=oracle["tests"][0]):
                response = answer(
                    [symbol(row["name"], row["file"], row["line"]) for row in oracle["symbols"]],
                    [test(oracle["tests"][0], oracle["tests"][0].split("::", 1)[0])],
                )

                self.assertTrue(grade_answer(response, oracle)["mechanical_pass"])

    def test_literal_locations_are_required(self):
        oracle = {
            "literals": [
                {"value": "account email required", "file": "config/registration.json", "line": 7},
                {"value": "/register", "file": "config/registration.json", "line": 3},
            ]
        }
        response = answer(
            literals=[
                {"value": "account email required", "file": "config/registration.json", "line": 7},
                {"value": "/register", "file": "config/registration.json", "line": 2},
            ]
        )

        result = grade_answer(response, oracle)

        self.assertFalse(result["mechanical_pass"])
        self.assertTrue(any("/register" in failure for failure in result["failures"]))


if __name__ == "__main__":
    unittest.main()
