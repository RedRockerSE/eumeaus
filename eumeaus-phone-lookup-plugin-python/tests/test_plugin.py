"""Unit tests for plugin.py's check() logic, run directly against the
generated protobuf types — no running gRPC server, no eumeaus-plugin-host,
no network, matching the pattern the Rust plugins' own tests/check.rs use
(plugin-developer-guide.md §9: "unit test your check() logic directly").

Run with: .venv/bin/python -m pytest tests/
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.realpath(__file__))))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.realpath(__file__))), "pb"))

import plugin_pb2  # noqa: E402
from plugin import check  # noqa: E402


def make_request(value: str) -> "plugin_pb2.CheckRequest":
    return plugin_pb2.CheckRequest(input_entity_type="PhoneNumber", input_value=value)


def test_valid_mobile_number_is_found_with_self_and_location_entities():
    results = check(make_request("+14155552671"))

    assert len(results) == 1
    result = results[0]
    assert result.status == plugin_pb2.FOUND

    by_type = {e.entity_type: e for e in result.entities}
    assert "PhoneNumber" in by_type
    assert by_type["PhoneNumber"].canonical_key == "+14155552671"
    assert by_type["PhoneNumber"].attributes["valid"] == "true"
    assert by_type["PhoneNumber"].attributes["region_code"] == "US"

    assert "Location" in by_type
    assert by_type["Location"].canonical_key == "US"

    location_rel = next(
        r for r in result.relationships if r.relationship_type == "LocatedAt"
    )
    assert location_rel.from_canonical_key == "+14155552671"
    assert location_rel.to_canonical_key == "US"


def test_valid_number_with_a_resolvable_carrier_emits_an_organization():
    # A Swedish mobile prefix libphonenumber's bundled data attributes to a
    # named carrier (unlike many US numbers, which is why the mobile test
    # above doesn't assert on this).
    results = check(make_request("+46701234567"))

    result = results[0]
    assert result.status == plugin_pb2.FOUND
    by_type = {e.entity_type: e for e in result.entities}
    assert "Organization" in by_type
    assert by_type["Organization"].attributes["role"] == "carrier"

    org_rel = next(
        r for r in result.relationships if r.relationship_type == "AssociatedWith"
    )
    assert org_rel.from_canonical_key == "+46701234567"
    assert org_rel.to_canonical_key == by_type["Organization"].canonical_key


def test_well_formed_but_impossible_number_is_not_found():
    results = check(make_request("+10000000000"))

    assert len(results) == 1
    assert results[0].status == plugin_pb2.NOT_FOUND
    assert len(results[0].entities) == 0
    assert len(results[0].relationships) == 0


def test_unparseable_input_is_an_error_not_a_crash():
    results = check(make_request("definitely not a phone number"))

    assert len(results) == 1
    assert results[0].status == plugin_pb2.ERROR
    assert results[0].error_message != ""


def test_every_result_carries_provenance():
    for value in ["+14155552671", "+10000000000", "garbage"]:
        for result in check(make_request(value)):
            assert result.HasField("provenance")
            assert result.provenance.plugin_name == "phone-lookup"
            assert result.provenance.retrieval_method == (
                "offline libphonenumber metadata lookup"
            )
