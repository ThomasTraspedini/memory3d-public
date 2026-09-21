import json
import subprocess
import sys

import pytest


QUERY = "What should I remember before changing token refresh?"

NODES = [
    ("component", "Token refresh implementation and rotation flow", 1.0),
    ("component", "AuthService owns authentication lifecycle", 0.9),
    ("component", "MobileApp maintains signed-in user sessions", 0.9),
    ("incident", "Mobile session loss incident after refresh rollout", 1.0),
    ("component", "Checkout requires authenticated user state", 0.8),
    ("policy", "RetryPolicy controls bounded request retries", 0.7),
    ("risk", "Network instability disrupts mobile requests", 0.9),
    ("decision", "Tokens rotate without storing plaintext credentials", 0.9),
    ("dependency", "Session cache stores short-lived authentication state", 0.8),
    ("operation", "Alert when refresh failure rate exceeds threshold", 0.8),
    ("test", "Mobile refresh regression suite covers expired tokens", 0.8),
    ("risk", "Clock skew can invalidate otherwise current tokens", 0.7),
    ("component", "ApiGateway validates access tokens", 0.8),
    ("component", "UserProfile serves customer preferences", 0.5),
    ("component", "Inventory tracks warehouse stock", 0.5),
    ("component", "TaxCalculator computes regional tax", 0.5),
    ("incident", "Search indexing lag delayed product discovery", 0.4),
    ("decision", "Audit logs retain security events for ninety days", 0.6),
    ("operation", "Nightly backups verify restore checksums", 0.6),
    ("component", "EmailWorker sends transactional messages", 0.4),
    ("risk", "Payment provider latency may delay checkout", 0.5),
    ("component", "RecommendationEngine ranks catalog items", 0.4),
    ("preference", "Prefer reversible database migrations", 0.7),
    ("decision", "Feature flags guard risky production rollouts", 0.7),
    ("component", "ImagePipeline produces catalog thumbnails", 0.3),
    ("operation", "Support dashboard summarizes open incidents", 0.4),
    ("test", "Contract tests pin payment provider responses", 0.5),
    ("risk", "Queue saturation can postpone email delivery", 0.4),
]

RELATIONS = [
    (1, 0, "owns", 1.0),
    (2, 0, "depends_on", 0.95),
    (0, 3, "caused", 1.0),
    (4, 2, "depends_on", 0.9),
    (5, 6, "mitigates", 0.9),
    (0, 8, "uses", 0.85),
    (0, 9, "monitored_by", 0.8),
    (0, 10, "verified_by", 0.9),
    (0, 11, "threatened_by", 0.75),
    (12, 0, "validates", 0.8),
    (3, 10, "prevented_by", 0.85),
    (2, 6, "affected_by", 0.7),
    (6, 5, "handled_by", 0.9),
    (4, 20, "threatened_by", 0.6),
    (20, 26, "covered_by", 0.8),
    (19, 27, "affected_by", 0.7),
    (23, 3, "guards_rollout_of", 0.65),
    (17, 9, "feeds", 0.55),
    (3, 25, "documented_in", 0.8),
    (8, 7, "constrained_by", 0.75),
]


def run_python(code, db_path):
    output = subprocess.run(
        [sys.executable, "-c", code, str(db_path)],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return json.loads(output.stdout)


def run_cli(db_path, *arguments):
    output = subprocess.run(
        [
            "cargo",
            "run",
            "-q",
            "-p",
            "memory3d-cli",
            "--",
            "--db",
            str(db_path),
            "--json",
            *arguments,
        ],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return json.loads(output.stdout)


def result_shape(report):
    return {
        "results": [
            {
                "node": result.node.as_dict(),
                "score": result.score,
                "hops": result.hops,
                "path": {
                    "seed": result.path.seed,
                    "steps": [
                        {
                            "source": step.source,
                            "relation": step.relation,
                            "weight": step.weight,
                            "target": step.target,
                        }
                        for step in result.path.steps
                    ],
                },
            }
            for result in report.results
        ],
        "stats": {
            "seeds": report.stats.seeds,
            "visited_nodes": report.stats.visited_nodes,
            "visited_edges": report.stats.visited_edges,
        },
        "database": {
            "seed_queries": report.database.seed_queries,
            "seed_node_reads": report.database.seed_node_reads,
            "adjacency_queries": report.database.adjacency_queries,
            "traversal_node_reads": report.database.traversal_node_reads,
        },
    }


INGEST_SCRIPT = f"""
import json
import sys
from memory3d import Memory

nodes = {NODES!r}
relations = {RELATIONS!r}

with Memory.open(sys.argv[1]) as memory:
    stored = [
        memory.add_text(kind, text, importance, metadata={{"fixture": "public-demo"}})
        for kind, text, importance in nodes
    ]
    for source, target, relation, weight in relations:
        memory.link(stored[source].id, stored[target].id, relation, weight)
    print(json.dumps({{
        "node_count": len(stored),
        "relation_count": len(relations),
        "first_id": stored[0].id,
        "metadata": stored[0].metadata,
        "closed_inside": memory.closed,
    }}))
"""


REOPEN_SCRIPT = f"""
import json
import sys
from memory3d import ClosedError, Memory

query = {QUERY!r}

memory = Memory.open(sys.argv[1])
first = memory.get(1)
appended = memory.add_text("note", "Appended from a fresh Python process", 0.5)
report = memory.activate(
    query,
    hops=3,
    seed_limit=1,
    limit=10,
    max_visited_nodes=64,
    max_visited_edges=128,
)
memory.close()
closed_error = None
try:
    memory.search("token")
except ClosedError as error:
    closed_error = str(error)

def shape(report):
    return {{
        "results": [
            {{
                "node": result.node.as_dict(),
                "score": result.score,
                "hops": result.hops,
                "path": {{
                    "seed": result.path.seed,
                    "steps": [
                        {{
                            "source": step.source,
                            "relation": step.relation,
                            "weight": step.weight,
                            "target": step.target,
                        }}
                        for step in result.path.steps
                    ],
                }},
            }}
            for result in report.results
        ],
        "stats": {{
            "seeds": report.stats.seeds,
            "visited_nodes": report.stats.visited_nodes,
            "visited_edges": report.stats.visited_edges,
        }},
        "database": {{
            "seed_queries": report.database.seed_queries,
            "seed_node_reads": report.database.seed_node_reads,
            "adjacency_queries": report.database.adjacency_queries,
            "traversal_node_reads": report.database.traversal_node_reads,
        }},
    }}

print(json.dumps({{
    "first_id": first.id,
    "first_text": first.text,
    "appended_id": appended.id,
    "closed_error": closed_error,
    "activation": shape(report),
}}))
"""


def test_ingest_reopen_append_activate_and_match_cli_fixture(tmp_path):
    db_path = tmp_path / "codebase.memory3d"

    ingest = run_python(INGEST_SCRIPT, db_path)
    assert ingest["node_count"] >= 25
    assert ingest["relation_count"] >= 5
    assert ingest["first_id"] == 1
    assert ingest["metadata"] == {"fixture": "public-demo"}
    assert not ingest["closed_inside"]
    assert db_path.is_file()

    reopened = run_python(REOPEN_SCRIPT, db_path)
    assert reopened["first_id"] == ingest["first_id"]
    assert reopened["first_text"] == NODES[0][1]
    assert reopened["appended_id"] > ingest["first_id"]
    assert "closed" in reopened["closed_error"]
    assert any(
        "Mobile session loss incident" in result["node"]["text"]
        for result in reopened["activation"]["results"]
    )
    assert any(result["hops"] >= 2 for result in reopened["activation"]["results"])

    cli_db_path = tmp_path / "cli.memory3d"
    run_cli(cli_db_path, "demo", "ingest")
    cli_recall = run_cli(cli_db_path, "demo", "recall")
    assert [
        result["node"]["id"] for result in reopened["activation"]["results"]
    ] == [result["node"]["id"] for result in cli_recall["results"]]
    assert [
        result["path"] for result in reopened["activation"]["results"]
    ] == [result["path"] for result in cli_recall["results"]]
    assert reopened["activation"]["stats"] == cli_recall["stats"]
    assert reopened["activation"]["database"] == cli_recall["database"]


def test_python_exceptions_are_typed_and_do_not_dump_stored_text(tmp_path):
    import memory3d

    db_path = tmp_path / "errors.memory3d"
    with memory3d.Memory.open(db_path) as memory:
        source = memory.add_text("component", "Sensitive token internals", 1.0)
        with pytest.raises(memory3d.ValidationError):
            memory.add_text("", "blank kind is rejected")
        with pytest.raises(memory3d.NotFoundError) as error:
            memory.link(source.id, 999_999, "depends_on")
    assert "Sensitive token internals" not in str(error.value)


def test_basic_api_surface_in_one_process(tmp_path):
    import memory3d

    assert memory3d.__version__ == "0.1.0"
    db_path = tmp_path / "api.memory3d"
    with memory3d.Memory(db_path) as memory:
        seed = memory.add_text(
            "component",
            "Token refresh component",
            1.0,
            metadata={"owner": "auth"},
            coordinates=(1.0, 2.0, 3.0),
        )
        target = memory.add_text("incident", "Mobile session loss", 1.0)
        relation = memory.link(seed.id, target.id, "caused", 0.8)
        assert relation.source == seed.id
        assert memory.get(seed.id).metadata == {"owner": "auth"}
        assert memory.get(seed.id).coordinates.z == 3.0
        assert memory.search("token refresh", limit=1)[0].node.id == seed.id
        report = memory.activate("token refresh", seed_limit=1, limit=1)
        assert result_shape(report)["results"][0]["path"]["steps"][0]["relation"] == "caused"
    assert memory.closed


def test_demo_scripts_are_importable_python(tmp_path):
    import pathlib

    root = pathlib.Path(__file__).resolve().parents[3]
    ingest = root / "examples" / "python-demo" / "ingest.py"
    recall = root / "examples" / "python-demo" / "recall.py"
    db_path = tmp_path / "demo.memory3d"
    first = subprocess.run(
        [sys.executable, str(ingest), str(db_path)],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    )
    second = subprocess.run(
        [sys.executable, str(recall), str(db_path)],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    )
    assert json.loads(first.stdout)["node_count"] >= 25
    assert any(
        "Mobile session loss incident" in result["text"]
        for result in json.loads(second.stdout)["results"]
    )
