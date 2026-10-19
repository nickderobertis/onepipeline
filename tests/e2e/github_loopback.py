"""Loopback GitHub GraphQL for the GitHub Projects write-back journeys.

Every answer is built from current endpoint state. Unknown operations fail loudly;
no request reaches GitHub or reads a production credential. The board carries a
`Host` text field a source's `metadata_fields` can project onto, and an optional
second argument names a JSON file of caller metadata to seed onto issues by name.

`GITHUB_LOOPBACK_ISSUES` names a JSON file of numbered issues and the boards that hold them,
for the journeys about the issues a landing closes: `{"repository": "owner/name", "issues":
{"7": {"state": "OPEN", "reason": null}, ...}, "boards": {"1": {"7": "Todo"}, ...}}`. Those
issues are read by repository and number, as `task status complete` reads them, and a board's
source names its board in its endpoint, `/board/<number>/graphql`, so every request is logged
under the board that sent it. POST `/fault` with `{"status": "refuse" | "transient" | "hold"}`
answers the next Status write of a numbered card that way: refused for good, failed once with
an HTTP 502 and then held until POST `/release`, or held until `/release` and then dropped
unapplied — the request a stopped driver sent and never saw answered.

The URL is published to `<root>/endpoint` whole or not at all. For the regression journey that
proves it, `GITHUB_LOOPBACK_PUBLISH_HOLD` names a directory: publication stops half-written,
creates `held` there, and finishes only once `go` appears.
"""
import json
import os
import pathlib
import re
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

root = pathlib.Path(sys.argv[1])
seeded = json.loads(pathlib.Path(sys.argv[2]).read_text()) if len(sys.argv) > 2 else {}
if (not isinstance(seeded, dict)
        or set(seeded) - {"plan", "anchor", "work", "other"}
        or not all(isinstance(metadata, dict) and all(isinstance(key, str) for key in metadata)
                   and not any(key.startswith(("onepipeline.", "onetaskgraph.")) for key in metadata)
                   for metadata in seeded.values())):
    raise SystemExit("seeded metadata must map issue names to objects of caller keys")
options = ["Todo", "Queued", "In Progress", "Needs Attention", "Done", "Canceled"]
host = {"__typename": "ProjectV2Field", "id": "HOST", "name": "Host", "dataType": "TEXT"}


def connection(nodes):
    return {"nodes": nodes, "pageInfo": {"hasNextPage": False, "endCursor": None}}


def field():
    return {"__typename": "ProjectV2SingleSelectField", "id": "STATUS", "name": "Status",
            "options": [{"id": name, "name": name} for name in options]}


def described(content, metadata):
    return content + "\n\n<!-- onetaskgraph.metadata\n" + json.dumps(metadata) + "\n-->"


state = {"issues": {}, "requests": []}
for name in ["plan", "anchor", "work", "other"]:
    metadata = ({"onepipeline.schema_version": 3, "onepipeline.concurrency": 1,
                 "onepipeline.goal": {"text": "Keep people's board edits"}}
                if name == "plan" else
                {"onepipeline.id": name, "onepipeline.persona": "engineer", **seeded.get(name, {})})
    content = "" if name == "plan" else (
        f"## What\nDo {name}.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- {name} is done.")
    state["issues"][name] = {"body": described(content, metadata), "status": "Todo", "state": "OPEN",
                              "title": "github-edits" if name == "plan" else name, "host": None}


def issue(name):
    held = state["issues"][name]
    values = [{"name": held["status"], "optionId": held["status"], "field": field()}]
    if held["host"] is not None:
        values.append({"text": held["host"], "field": {"id": host["id"], "name": host["name"]}})
    membership = {"id": "ITEM-" + name, "project": {"id": "BOARD", "number": 1},
                  "fieldValues": connection(values)}
    value = {"__typename": "Issue", "id": name, "title": held["title"],
             "body": held["body"], "state": held["state"], "stateReason": None, "number": 1,
             "url": None, "parent": None if name == "plan" else {"id": "plan"},
             "subIssuesSummary": {"total": 3 if name == "plan" else 0},
             "labels": connection([]), "projectItems": connection([membership]),
             "blockedBy": connection([]), "blocking": connection([])}
    if name == "plan":
        value["subIssues"] = connection([issue("anchor"), issue("work"), issue("other")])
    return value


numbered = (json.loads(pathlib.Path(os.environ["GITHUB_LOOPBACK_ISSUES"]).read_text())
            if os.environ.get("GITHUB_LOOPBACK_ISSUES") else
            {"repository": None, "issues": {}, "boards": {}})
state["numbered"] = numbered
state["log"] = []
state["writes"] = []
state["fault"] = None
state["holding"] = False
# One lock over every read and write of state, since a held write waits on its own thread.
lock = threading.Lock()
released = threading.Event()


def numbered_status_field(board):
    return {"id": "STATUS-" + board, "name": "Status",
            "options": [{"id": name, "name": name} for name in options]}


def numbered_issue(number):
    """The `HeldIssue` an issue read by reference answers: its every board membership, each
    carrying its card's `Status` option by name."""
    held = numbered["issues"][number]
    owner, name = numbered["repository"].split("/")
    memberships = [
        {"id": "ITEM-" + board + "-" + number, "updatedAt": "2026-10-01T00:00:00Z",
         "project": {"id": "BOARD-" + board, "number": int(board)},
         "status": {"name": cards[number], "field": numbered_status_field(board)}}
        for board, cards in sorted(numbered["boards"].items()) if number in cards]
    return {"__typename": "Issue", "id": "ISSUE-" + number, "number": int(number),
            "title": "issue " + number, "body": "Issue " + number + ".",
            "url": "https://github.com/" + numbered["repository"] + "/issues/" + number,
            "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z",
            "state": held["state"], "stateReason": held["reason"],
            "repository": {"nameWithOwner": numbered["repository"]}, "parent": None,
            "subIssuesSummary": {"total": 0}, "commentCount": {"totalCount": 0},
            "relationshipEvents": {"totalCount": 0},
            "projectItems": {"nodes": memberships,
                             "pageInfo": {"hasNextPage": False, "endCursor": None}}}


def issues_by_reference(variables):
    data, errors = {}, []
    slot = 0
    while "owner%d" % slot in variables:
        repository = "%s/%s" % (variables["owner%d" % slot], variables["name%d" % slot])
        number = str(variables["number%d" % slot])
        if repository != numbered["repository"]:
            data["r%d" % slot] = None
            errors.append({"type": "NOT_FOUND", "path": ["r%d" % slot],
                           "message": "Could not resolve to a Repository"})
        elif number not in numbered["issues"]:
            data["r%d" % slot] = {"issue": None}
            errors.append({"type": "NOT_FOUND", "path": ["r%d" % slot, "issue"],
                           "message": "Could not resolve to an issue"})
        else:
            data["r%d" % slot] = {"issue": numbered_issue(number)}
        slot += 1
    answer = {"data": data}
    if errors:
        answer["errors"] = errors
    return answer


NUMBERED_ITEM = re.compile(r"^ITEM-(\d+)-(\d+)$")


def write_numbered_status(supplied):
    """A Status write of a numbered card: what the fault says, or the card moved."""
    matched = NUMBERED_ITEM.match(supplied.get("itemId", ""))
    board, number = matched.groups()
    if (set(supplied) != {"projectId", "itemId", "fieldId", "value"}
            or supplied["projectId"] != "BOARD-" + board
            or supplied["fieldId"] != "STATUS-" + board
            or number not in numbered["boards"].get(board, {})
            or not isinstance(supplied["value"], dict)
            or supplied["value"].get("singleSelectOptionId") not in options):
        raise ValueError("invalid numbered Status mutation input")
    state["writes"].append({"board": int(board), "issue": int(number), "field": "Status",
                            "value": supplied["value"]["singleSelectOptionId"]})
    numbered["boards"][board][number] = supplied["value"]["singleSelectOptionId"]
    return {"data": {"updateProjectV2ItemFieldValue": {"projectV2Item": {"id": supplied["itemId"]}}}}


def write_field(supplied, clear=False):
    keys = {"projectId", "itemId", "fieldId"} if clear else {"projectId", "itemId", "fieldId", "value"}
    if (not isinstance(supplied, dict) or set(supplied) != keys
            or supplied["projectId"] != "BOARD"
            or supplied["itemId"] not in ["ITEM-" + name for name in state["issues"]]):
        raise ValueError("invalid field mutation input")
    held = state["issues"][supplied["itemId"].removeprefix("ITEM-")]
    if supplied["fieldId"] == host["id"]:
        if clear:
            held["host"] = None
        elif (isinstance(supplied["value"], dict) and set(supplied["value"]) == {"text"}
                and isinstance(supplied["value"]["text"], str)):
            held["host"] = supplied["value"]["text"]
        else:
            raise ValueError("invalid Host mutation input")
    elif (supplied["fieldId"] == "STATUS" and not clear
            and isinstance(supplied["value"], dict)
            and set(supplied["value"]) == {"singleSelectOptionId"}
            and supplied["value"]["singleSelectOptionId"] in options):
        held["status"] = supplied["value"]["singleSelectOptionId"]
    else:
        raise ValueError("invalid Status mutation input")
    return {"projectV2Item": {"id": supplied["itemId"]}}



class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, answer, status=200):
        encoded = json.dumps(answer).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_GET(self):
        if self.path != "/state":
            self.reply({"errors": [{"message": "unknown route"}]}, 404)
            return
        with lock:
            self.reply(state)

    def held_write(self, request):
        """The Status write a fault holds: answered only once `/release` is posted."""
        fault = state["fault"]
        if fault == "refuse":
            state["fault"] = None
            return {"errors": [{"type": "UNPROCESSABLE",
                                "message": "the loopback refuses this Status write"}]}, 200
        if fault == "transient":
            state["fault"] = "hold-then-apply"
            return {"errors": [{"message": "the loopback is failing for now"}]}, 502
        if fault in ("hold", "hold-then-apply"):
            state["fault"] = None
            state["holding"] = True
            lock.release()
            try:
                released.wait()
            finally:
                lock.acquire()
            state["holding"] = False
            if fault == "hold":
                return {"errors": [{"message": "the loopback dropped a held write"}]}, 502
        return write_numbered_status(request["variables"]["input"]), 200

    def do_POST(self):
        try:
            length = int(self.headers.get("Content-Length", ""))
            if not 0 < length <= 262144:
                raise ValueError("Content-Length must be between 1 and 262144")
            request = json.loads(self.rfile.read(length))
            if not isinstance(request, dict):
                raise ValueError("request must be an object")
            if self.path == "/fault":
                if set(request) != {"status"} or request["status"] not in ["refuse", "transient", "hold"]:
                    raise ValueError("a fault names a Status write's fault")
                with lock:
                    state["fault"] = request["status"]
                answer = {"data": {}}
            elif self.path == "/release":
                released.set()
                answer = {"data": {}}
            elif self.path == "/human":
                if set(request) != {"body", "status"} or not isinstance(request["body"], str):
                    raise ValueError("human edit must contain only a body string and a status")
                if request["status"] not in options:
                    raise ValueError("unknown human status")
                state["issues"]["work"].update(request)
                answer = {"data": {}}
            elif self.path == "/graphql" or re.fullmatch(r"/board/\d+/graphql", self.path):
                with lock:
                    answer, status = self.graphql(request)
                self.reply(answer, status)
                return
            else:
                raise ValueError("unknown route")
        except (ValueError, TypeError, KeyError) as error:
            self.reply({"errors": [{"message": str(error)}]}, 400)
            return
        self.reply(answer)

    def graphql(self, request):
        query = request.get("query")
        variables = request.get("variables")
        if (not isinstance(query, str) or not query.strip()
                or not isinstance(variables, dict)
                or set(request) - {"query", "variables", "operationName"}
                or (request.get("operationName") is not None
                    and not isinstance(request["operationName"], str))):
            raise ValueError("GraphQL requires a query string and an object of variables")
        state["requests"].append(request)
        state["log"].append({"path": self.path, "query": query, "variables": variables})
        answer = self.numbered(query, variables, request)
        if answer is not None:
            return answer
        return self.named(query, variables), 200

    def numbered(self, query, variables, request):
        """The numbered issues' operations, or `None` for one about the named ones."""
        if "r0:repository(owner:$owner0" in query:
            return issues_by_reference(variables), 200
        supplied = variables.get("input") if isinstance(variables.get("input"), dict) else {}
        if "updateProjectV2ItemFieldValue(" in query and NUMBERED_ITEM.match(str(supplied.get("itemId", ""))):
            return self.held_write(request)
        if "updateIssue(" in query and str(supplied.get("id", "")).startswith("ISSUE-"):
            number = supplied["id"].removeprefix("ISSUE-")
            movement = supplied.get("stateInput")
            if (number not in numbered["issues"] or set(supplied) != {"id", "stateInput"}
                    or not isinstance(movement, dict)
                    or movement.get("value") not in ["OPEN", "CLOSED"]
                    or set(movement) - {"value", "stateReason"}):
                raise ValueError("invalid numbered issue mutation input")
            numbered["issues"][number] = {"state": movement["value"],
                                          "reason": movement.get("stateReason")}
            state["writes"].append({"issue": int(number), "state": movement["value"],
                                    "reason": movement.get("stateReason")})
            return {"data": {"updateIssue": {"issue": {"id": supplied["id"]}}}}, 200
        return None

    def named(self, query, variables):
        if "updateIssue(" in query:
            supplied = variables.get("input")
            if (not isinstance(supplied, dict) or not isinstance(supplied.get("id"), str)
                    or supplied["id"] not in state["issues"]
                    or set(supplied) - {"id", "body", "title", "stateInput"}):
                raise ValueError("invalid issue mutation input")
            for key in ["body", "title"]:
                if key in supplied and not isinstance(supplied[key], str):
                    raise ValueError(key + " must be a string")
            movement = supplied.get("stateInput")
            if movement is not None and (not isinstance(movement, dict)
                    or movement.get("value") not in ["OPEN", "CLOSED"]
                    or set(movement) - {"value", "stateReason"}
                    or movement.get("stateReason") not in [None, "COMPLETED", "NOT_PLANNED"]):
                raise ValueError("invalid issue state input")
            held = state["issues"][supplied["id"]]
            if "title" in supplied:
                held["title"] = supplied["title"]
            if "body" in supplied:
                held["body"] = supplied["body"]
            if movement is not None:
                held["state"] = movement["value"]
            return {"data": {"updateIssue": {"issue": {"id": supplied["id"]}}}}
        if "second:updateProjectV2ItemFieldValue(" in query:
            raise ValueError("the batched field update is not served here")
        if "updateProjectV2ItemFieldValue(" in query:
            return {"data": {"updateProjectV2ItemFieldValue": write_field(variables.get("input"))}}
        if "clearProjectV2ItemFieldValue(" in query:
            return {"data": {"clearProjectV2ItemFieldValue": write_field(variables.get("input"), True)}}
        if "node(id:" in query:
            name = variables.get("id")
            if not isinstance(name, str) or name not in state["issues"]:
                raise ValueError("unknown issue id")
            return {"data": {"node": issue(name)}}
        if "repositoryOwner(" in query:
            board = {"id": "BOARD", "title": "Board", "fields": connection([field(), host]),
                     "items": connection([{"id": "ITEM-" + name, "content": issue(name),
                                            "fieldValues": issue(name)["projectItems"]["nodes"][0]["fieldValues"]}
                                           for name in state["issues"]])}
            alias = "boardFields" if "boardFields:" in query else "owner"
            return {"data": {alias: {"projectV2": board}}}
        return {"errors": [{"message": "unsupported loopback operation: " + query}]}


def publish(path, text):
    """Write `text` under another name and rename it onto `path`, so a reader that finds `path`
    reads all of `text`."""
    staged = path.with_name(path.name + ".partial")
    hold = os.environ.get("GITHUB_LOOPBACK_PUBLISH_HOLD")
    with open(staged, "w") as out:
        if hold:
            out.write(text[:len(text) // 2])
            out.flush()
            (pathlib.Path(hold) / "held").touch()
            while not (pathlib.Path(hold) / "go").exists():
                time.sleep(0.01)
            text = text[len(text) // 2:]
        out.write(text)
    os.replace(staged, path)


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
# Published by a rename, so a reader that sees `endpoint` sees the whole address: a
# file written in place exists, empty, before its contents land, and a journey that
# read it then sent its first request to a bare path.
publish(root / "endpoint", f"http://127.0.0.1:{server.server_port}")
server.serve_forever()
