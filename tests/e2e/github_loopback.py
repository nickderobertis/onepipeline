"""Loopback GitHub GraphQL for the GitHub Projects write-back journeys.

Every answer is built from current endpoint state. Unknown operations fail loudly;
no request reaches GitHub or reads a production credential. The board carries a
`Host` text field a source's `metadata_fields` can project onto, and an optional
second argument names a JSON file of caller metadata to seed onto issues by name.

`GITHUB_LOOPBACK_ISSUES` names a JSON file of numbered issues and the boards that hold them,
for the journeys about the issues a landing closes: `{"repository": "owner/name", "issues":
{"7": {"state": "OPEN", "reason": null, "delivers": ["source:ticket"], "subissues": 2}, ...},
"boards": {"1": {"7": "Todo"}, ...}}`, `delivers` optional and written into the issue's metadata
slot, and `subissues` optional, the count of sub-issues that makes an issue a plan's home. A read
of an `ISSUE-<n>` this endpoint holds no issue for fails with an HTTP 502. Those
issues are read by repository and number, as `task status complete` reads them, and a board's
source names its board in its endpoint, `/board/<number>/graphql`, so every request is logged
under the board that sent it. POST `/fault` with `{"status": FAULT}`, or a list of them for
successive writes, answers the next Status write of a numbered card that way: `refuse`d for
good, `fail-once` with an HTTP 502, `transient` — failed once with an HTTP 502 and the retry
then held until POST `/release` — or `hold` until `/release` and then dropped unapplied, the
request a stopped driver sent and never saw answered. POST `/reads` with `{"reads": ["pass" |
"refuse", ...]}` answers the next reads of issues by reference that way, one each, in order.

`GITHUB_LOOPBACK_PLAN` names a JSON file reshaping the named board's plan, for the journeys about
a run's home: `{"tasks": {"first": [], "second": ["first"]}, "status": "In Progress"}` — each task
by name with the tasks it is blocked by, in order, and the home's own Status option; without it
the plan holds `anchor`, `work` and `other`, none blocked, at `Todo`. POST `/home-fault` with
`{"fault": "refuse" | "fail" | "pass"}` answers every later Status write of the home that way:
`refuse`d for good, `fail`ed with an HTTP 502, or taken.

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
shaped = (json.loads(pathlib.Path(os.environ["GITHUB_LOOPBACK_PLAN"]).read_text())
          if os.environ.get("GITHUB_LOOPBACK_PLAN") else
          {"tasks": {"anchor": [], "work": [], "other": []}, "status": "Todo"})
if (not isinstance(shaped, dict) or set(shaped) != {"tasks", "status"}
        or not isinstance(shaped["tasks"], dict) or not shaped["tasks"]
        or "plan" in shaped["tasks"]
        or not all(isinstance(blocked, list) and all(by in shaped["tasks"] and by != task
                                                     for by in blocked)
                   for task, blocked in shaped["tasks"].items())):
    raise SystemExit("a plan names its tasks, each with the tasks it is blocked by, and a status")
TASKS = list(shaped["tasks"])
NAMES = ["plan"] + TASKS
if (not isinstance(seeded, dict)
        or set(seeded) - set(NAMES)
        or not all(isinstance(metadata, dict) and all(isinstance(key, str) for key in metadata)
                   and not any(key.startswith(("onepipeline.", "onetaskgraph.")) for key in metadata)
                   for metadata in seeded.values())):
    raise SystemExit("seeded metadata must map issue names to objects of caller keys")
options = ["Todo", "Queued", "In Progress", "Needs Attention", "Done", "Canceled", "Shipped"]
if shaped["status"] not in options:
    raise SystemExit("the plan's status must be a Status option")
host = {"__typename": "ProjectV2Field", "id": "HOST", "name": "Host", "dataType": "TEXT"}


def connection(nodes):
    return {"nodes": nodes, "pageInfo": {"hasNextPage": False, "endCursor": None}}


def field():
    return {"__typename": "ProjectV2SingleSelectField", "id": "STATUS", "name": "Status",
            "options": [{"id": name, "name": name} for name in options]}


def described(content, metadata):
    return content + "\n\n<!-- onetaskgraph.metadata\n" + json.dumps(metadata) + "\n-->"


state = {"issues": {}, "requests": [], "home_fault": "pass"}
for name in NAMES:
    metadata = ({"onepipeline.schema_version": 3, "onepipeline.concurrency": 1,
                 "onepipeline.goal": {"text": "Keep people's board edits"}}
                if name == "plan" else
                {"onepipeline.id": name, "onepipeline.persona": "engineer", **seeded.get(name, {})})
    content = "" if name == "plan" else (
        f"## What\nDo {name}.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- {name} is done.")
    state["issues"][name] = {"body": described(content, metadata),
                             "status": shaped["status"] if name == "plan" else "Todo",
                             "state": "OPEN", "reason": None,
                             "title": "github-edits" if name == "plan" else name, "host": None}


def related(name):
    """The far end of a dependency, as the `Related` fragment reads it."""
    held = state["issues"][name]
    return {"id": name, "title": held["title"], "body": held["body"], "parent": {"id": "plan"},
            "subIssuesSummary": {"total": 0}}


def issue(name):
    held = state["issues"][name]
    values = [{"name": held["status"], "optionId": held["status"], "field": field()}]
    if held["host"] is not None:
        values.append({"text": held["host"], "field": {"id": host["id"], "name": host["name"]}})
    membership = {"id": "ITEM-" + name, "project": {"id": "BOARD", "number": 1},
                  "fieldValues": connection(values)}
    blocked_by = shaped["tasks"].get(name, [])
    blocking = [task for task, blocked in shaped["tasks"].items() if name in blocked]
    value = {"__typename": "Issue", "id": name, "title": held["title"],
             "body": held["body"], "state": held["state"], "stateReason": held["reason"],
             "number": 1, "url": None, "parent": None if name == "plan" else {"id": "plan"},
             "subIssuesSummary": {"total": len(TASKS) if name == "plan" else 0},
             "labels": connection([]), "projectItems": connection([membership]),
             "blockedBy": connection([related(by) for by in blocked_by]),
             "blocking": connection([related(task) for task in blocking])}
    if name == "plan":
        value["subIssues"] = connection([issue(task) for task in TASKS])
    return value


STATES = {"OPEN": [None], "CLOSED": ["COMPLETED", "NOT_PLANNED", "DUPLICATE"]}
NUMBER = re.compile(r"^[1-9]\d*$")
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")


def numbered_fixture(document):
    """The numbered issues and boards a journey seeds, refused unless every part is one this
    endpoint can answer as GitHub would."""
    if (not isinstance(document, dict) or set(document) != {"repository", "issues", "boards"}
            or not isinstance(document["repository"], str)
            or not REPOSITORY.match(document["repository"])
            or not isinstance(document["issues"], dict)
            or not isinstance(document["boards"], dict)):
        raise SystemExit("numbered issues must name a repository, its issues and its boards")
    for number, held in document["issues"].items():
        if (not NUMBER.match(number) or not isinstance(held, dict)
                or not {"state", "reason"} <= set(held) <= {"state", "reason", "delivers",
                                                             "subissues"}
                or held["reason"] not in STATES.get(held["state"], [])
                or not isinstance(held.get("subissues", 0), int)
                or isinstance(held.get("subissues", 0), bool) or held.get("subissues", 0) < 0
                or not isinstance(held.get("delivers", []), list)
                or not all(isinstance(ticket, str) and ":" in ticket
                           for ticket in held.get("delivers", []))):
            raise SystemExit("issue %r is not a number with a state, the reason it fits and the "
                             "tickets it delivers" % number)
    for board, cards in document["boards"].items():
        if (not NUMBER.match(board) or not isinstance(cards, dict)
                or any(number not in document["issues"] or card not in options
                       for number, card in cards.items())):
            raise SystemExit("board %r must hold seeded issues at Status options" % board)
    return document


numbered = (numbered_fixture(json.loads(pathlib.Path(os.environ["GITHUB_LOOPBACK_ISSUES"]).read_text()))
            if os.environ.get("GITHUB_LOOPBACK_ISSUES") else
            {"repository": None, "issues": {}, "boards": {}})
state["numbered"] = numbered
state["log"] = []
state["writes"] = []
state["fault"] = None
state["holding"] = False
state["queued"] = []
state["reads"] = []
FAULTS = ["refuse", "fail-once", "transient", "hold"]


def next_fault():
    """The fault the next numbered Status write meets, once the current one has answered."""
    state["fault"] = state["queued"].pop(0) if state["queued"] else None


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
            "title": "issue " + number,
            "body": ("Issue " + number + "." if not held.get("delivers") else
                     described("Issue " + number + ".", {"onetaskgraph.delivers": held["delivers"]})),
            "url": "https://github.com/" + numbered["repository"] + "/issues/" + number,
            "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z",
            "state": held["state"], "stateReason": held["reason"],
            "repository": {"nameWithOwner": numbered["repository"]}, "parent": None,
            "subIssuesSummary": {"total": held.get("subissues", 0)},
            "commentCount": {"totalCount": 0},
            "relationshipEvents": {"totalCount": 0},
            "projectItems": {"nodes": memberships,
                             "pageInfo": {"hasNextPage": False, "endCursor": None}}}


def issues_by_reference(variables):
    data, errors = {}, []
    slot = 0
    while "owner%d" % slot in variables:
        owner, name = variables["owner%d" % slot], variables.get("name%d" % slot)
        number = variables.get("number%d" % slot)
        if (not isinstance(owner, str) or not isinstance(name, str)
                or not isinstance(number, int) or isinstance(number, bool) or number < 1):
            raise ValueError("slot %d of a read by reference names no owner, name and number" % slot)
        repository = "%s/%s" % (owner, name)
        number = str(number)
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
            or set(supplied["value"]) != {"singleSelectOptionId"}
            or supplied["value"]["singleSelectOptionId"] not in options):
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
        """The Status write the current fault answers, the next queued fault taking its place
        once it has — a held one answered only once `/release` is posted."""
        fault = state["fault"]
        if fault == "refuse":
            next_fault()
            return {"errors": [{"type": "UNPROCESSABLE",
                                "message": "the loopback refuses this Status write"}]}, 200
        if fault == "fail-once":
            next_fault()
            return {"errors": [{"message": "the loopback is failing for now"}]}, 502
        if fault == "transient":
            state["fault"] = "hold-then-apply"
            return {"errors": [{"message": "the loopback is failing for now"}]}, 502
        if fault in ("hold", "hold-then-apply"):
            next_fault()
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
            if self.path == "/reads":
                reads = request.get("reads") if set(request) == {"reads"} else None
                if (not isinstance(reads, list)
                        or any(read not in ["pass", "refuse"] for read in reads)):
                    raise ValueError("reads name how the next reads by reference are answered")
                with lock:
                    state["reads"] = reads
                answer = {"data": {}}
            elif self.path == "/fault":
                faults = request.get("status") if set(request) == {"status"} else None
                faults = [faults] if isinstance(faults, str) else faults
                if (not isinstance(faults, list) or not faults
                        or any(fault not in FAULTS for fault in faults)):
                    raise ValueError("a fault names the Status writes' faults, in order")
                with lock:
                    state["fault"], state["queued"] = faults[0], faults[1:]
                answer = {"data": {}}
            elif self.path == "/home-fault":
                fault = request.get("fault") if set(request) == {"fault"} else None
                if fault not in ["refuse", "fail", "pass"]:
                    raise ValueError("a home fault is refuse, fail or pass")
                with lock:
                    state["home_fault"] = fault
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
        answer = self.home_fault(query, variables)
        if answer is not None:
            return answer
        return self.named(query, variables), 200

    def home_fault(self, query, variables):
        """What a Status write of the home meets while `/home-fault` says other than `pass`."""
        supplied = variables.get("input") if isinstance(variables.get("input"), dict) else {}
        if (state["home_fault"] == "pass" or "updateProjectV2ItemFieldValue(" not in query
                or supplied.get("itemId") != "ITEM-plan" or supplied.get("fieldId") != "STATUS"):
            return None
        if state["home_fault"] == "refuse":
            return {"errors": [{"type": "UNPROCESSABLE",
                                "message": "the loopback refuses the home's Status write"}]}, 200
        return {"errors": [{"message": "the loopback is failing the home's write for now"}]}, 502

    def numbered(self, query, variables, request):
        """The numbered issues' operations, or `None` for one about the named ones."""
        if "r0:repository(owner:$owner0" in query:
            read = state["reads"].pop(0) if state["reads"] else "pass"
            if read == "refuse":
                return {"errors": [{"type": "FORBIDDEN",
                                    "message": "the loopback refuses this read by reference"}]}, 200
            return issues_by_reference(variables), 200
        read = variables.get("id")
        if ("node(id:" in query and isinstance(read, str) and read.startswith("ISSUE-")
                and read.removeprefix("ISSUE-") not in numbered["issues"]):
            # An issue this endpoint holds no record of: a ticket a seeded issue delivers,
            # whose read GitHub is failing to answer for now.
            return {"errors": [{"message": "the loopback cannot read this issue for now"}]}, 502
        supplied = variables.get("input") if isinstance(variables.get("input"), dict) else {}
        item, issue_id = supplied.get("itemId"), supplied.get("id")
        if ("updateProjectV2ItemFieldValue(" in query and isinstance(item, str)
                and NUMBERED_ITEM.match(item)):
            return self.held_write(request)
        if "updateIssue(" in query and isinstance(issue_id, str) and issue_id.startswith("ISSUE-"):
            number = supplied["id"].removeprefix("ISSUE-")
            movement = supplied.get("stateInput")
            if (number not in numbered["issues"] or set(supplied) != {"id", "stateInput"}
                    or not isinstance(movement, dict)
                    or set(movement) - {"value", "stateReason"}
                    or movement.get("value") not in STATES
                    or movement.get("stateReason") not in STATES[movement["value"]]):
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
                held["reason"] = movement.get("stateReason")
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
