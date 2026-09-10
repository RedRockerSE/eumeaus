#!/usr/bin/env python3
"""eumeaus-phone-lookup-plugin (Python PoC).

Implements the Eumeaus plugin wire protocol (SPEC.md §2.2/§3.2,
plugin-developer-guide.md §3) directly, with no SDK — this is the
proof-of-concept referenced there for "if you're not using the Rust SDK,
writing in Python, Go, or anything else."

Checks a PhoneNumber entity against Google's libphonenumber metadata (the
`phonenumbers` PyPI package, a pure-Python port) for validity, region, an
approximate carrier, and timezone(s) — entirely offline, no network call
and no API key, using the same bundled reference data behind Android/
Chromium's own phone number handling. `permissions.network = false` in
plugin.toml reflects this honestly.

Run directly by eumeaus-plugin-host as a subprocess (see `run`, this
directory's executable entrypoint) — not meant to be invoked by a human.
See README.md for setup and manual testing instructions.
"""

import os
import sys
import time
from concurrent import futures

_SCRIPT_DIR = os.path.dirname(os.path.realpath(__file__))
# The generated pb2 modules use flat (non-package) imports of each other
# (`import plugin_pb2 as plugin__pb2`), so pb/ must be on sys.path itself,
# not imported as a package (`from pb import plugin_pb2`).
sys.path.insert(0, os.path.join(_SCRIPT_DIR, "pb"))

import grpc  # noqa: E402
import phonenumbers  # noqa: E402
from phonenumbers import carrier, geocoder, timezone  # noqa: E402

import plugin_pb2  # noqa: E402
import plugin_pb2_grpc  # noqa: E402

PLUGIN_NAME = "phone-lookup"
PLUGIN_VERSION = "0.1.0"  # keep in sync with plugin.toml's [plugin] version

HANDSHAKE_MAGIC = "EUMEAUS-PLUGIN"
HANDSHAKE_CORE_VERSION = "1"


def _now_unix_ms() -> int:
    return int(time.time() * 1000)


def _provenance() -> "plugin_pb2.Provenance":
    return plugin_pb2.Provenance(
        source_url="",
        retrieval_method="offline libphonenumber metadata lookup",
        # No raw external response exists for a purely local computation —
        # left blank rather than hashing something that isn't one, same as
        # eumeaus-email-lookup-plugin leaves it blank on a transport error.
        raw_response_sha256="",
        collected_at_unix_ms=_now_unix_ms(),
        plugin_name=PLUGIN_NAME,
        plugin_version=PLUGIN_VERSION,
    )


def _error_result(message: str) -> "plugin_pb2.CheckResult":
    return plugin_pb2.CheckResult(
        status=plugin_pb2.ERROR,
        error_message=message,
        provenance=_provenance(),
    )


def check(request: "plugin_pb2.CheckRequest") -> list:
    """The plugin's actual collection logic — a list of `CheckResult`s for
    one `CheckRequest`. Separated from the gRPC servicer so it can be unit
    tested directly (see tests/test_plugin.py) without a running server.

    Never raises: a bad/unparseable input becomes an ERROR result, per
    plugin-developer-guide.md §3.3 ("your plugin should never panic or let
    a transport error propagate uncaught").
    """
    value = request.input_value
    try:
        parsed = phonenumbers.parse(value, None)
    except phonenumbers.NumberParseException as e:
        return [
            _error_result(
                f"could not parse {value!r} as a phone number ({e}); "
                "expected E.164 format, e.g. +14155552671"
            )
        ]
    except Exception as e:  # defensive: never let check() itself crash
        return [_error_result(f"unexpected error parsing {value!r}: {e}")]

    if not phonenumbers.is_valid_number(parsed):
        return [
            plugin_pb2.CheckResult(
                status=plugin_pb2.NOT_FOUND,
                provenance=_provenance(),
            )
        ]

    region = phonenumbers.region_code_for_number(parsed) or ""
    description = geocoder.description_for_number(parsed, "en")
    carrier_name = carrier.name_for_number(parsed, "en")
    timezones = timezone.time_zones_for_number(parsed)
    e164 = phonenumbers.format_number(parsed, phonenumbers.PhoneNumberFormat.E164)
    line_type = phonenumbers.PhoneNumberType.to_string(phonenumbers.number_type(parsed))

    entities = [
        # Self-enrichment: same entity_type + canonical_key as the scanned
        # target, so this auto-merges onto it instead of creating a new
        # entity — the pattern eumeaus-crypto-wallet-plugin uses (CLAUDE.md
        # / plugin-developer-guide.md §10).
        plugin_pb2.EntityFinding(
            entity_type="PhoneNumber",
            canonical_key=value,
            display_label=e164,
            attributes={
                "e164": e164,
                "valid": "true",
                "line_type": line_type,
                "region_code": region,
                "country_code": str(parsed.country_code),
            },
        )
    ]
    relationships = []

    if region or description:
        location_key = region or description
        entities.append(
            plugin_pb2.EntityFinding(
                entity_type="Location",
                canonical_key=location_key,
                display_label=description or region,
                attributes={
                    "region_code": region,
                    "description": description,
                    "timezones": ",".join(timezones),
                },
            )
        )
        relationships.append(
            plugin_pb2.RelationshipFinding(
                from_canonical_key=value,
                to_canonical_key=location_key,
                relationship_type="LocatedAt",
            )
        )

    if carrier_name:
        entities.append(
            plugin_pb2.EntityFinding(
                entity_type="Organization",
                canonical_key=carrier_name,
                display_label=carrier_name,
                attributes={"role": "carrier"},
            )
        )
        relationships.append(
            plugin_pb2.RelationshipFinding(
                from_canonical_key=value,
                to_canonical_key=carrier_name,
                relationship_type="AssociatedWith",
            )
        )

    return [
        plugin_pb2.CheckResult(
            status=plugin_pb2.FOUND,
            entities=entities,
            relationships=relationships,
            provenance=_provenance(),
        )
    ]


class PhoneLookupServicer(plugin_pb2_grpc.PluginRuntimeServicer):
    def Describe(self, request, context):
        return plugin_pb2.DescribeResponse(
            plugin_name=PLUGIN_NAME, plugin_version=PLUGIN_VERSION
        )

    def Check(self, request, context):
        # check() never raises, but a servicer method leaking an exception
        # would otherwise just kill this one RPC with an opaque UNKNOWN
        # status on the host side — an explicit ERROR CheckResult is the
        # honest, documented way to report a plugin-side failure.
        try:
            for result in check(request):
                yield result
        except Exception as e:  # pragma: no cover - defense in depth
            yield _error_result(f"unhandled plugin error: {e}")


def _write_handshake(network: str, address: str) -> None:
    # SPEC.md §2.2 / plugin-developer-guide.md §3.1: exactly one line to
    # stdout, then flush — the host reads this line-by-line and is
    # blocked waiting for it.
    print(f"{HANDSHAKE_MAGIC}|{HANDSHAKE_CORE_VERSION}|{network}|{address}|grpc", flush=True)


def main() -> None:
    if os.name != "posix":
        print(
            "eumeaus-phone-lookup-plugin-python only implements the Unix domain "
            "socket transport; Windows named-pipe support is out of scope for "
            "this proof-of-concept (see README.md).",
            file=sys.stderr,
        )
        sys.exit(1)

    plugin_dir = os.environ.get("EUMEAUS_PLUGIN_DIR")
    if not plugin_dir:
        print(
            "EUMEAUS_PLUGIN_DIR is not set — this plugin is expected to be "
            "spawned by eumeaus-plugin-host, not run directly.",
            file=sys.stderr,
        )
        sys.exit(1)

    socket_path = os.path.join(plugin_dir, "plugin.sock")
    try:
        os.remove(socket_path)
    except FileNotFoundError:
        pass

    server = grpc.server(futures.ThreadPoolExecutor(max_workers=8))
    plugin_pb2_grpc.add_PluginRuntimeServicer_to_server(PhoneLookupServicer(), server)
    server.add_insecure_port(f"unix:{socket_path}")
    server.start()

    # The handshake's <address> field is the bare filesystem path — the
    # host connects with a raw UnixStream::connect(address), not a
    # "unix:"-scheme URI (that scheme is only meaningful to grpc's own
    # add_insecure_port/insecure_channel above).
    _write_handshake("unix", socket_path)

    server.wait_for_termination()


if __name__ == "__main__":
    main()
