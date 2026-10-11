"""Loopback GitHub GraphQL for the GitHub Projects write-back journeys.

Every answer is built from current endpoint state. Unknown operations fail loudly;
no request reaches GitHub or reads a production credential. The board carries a
`Host` text field a source's `metadata_fields` can project onto, and an optional
second argument names a JSON file of caller metadata to seed onto issues by name.
"""
import json
import pathlib
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

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


def save():
    staged = root / "state.next"
    staged.write_text(json.dumps(state))
    staged.replace(root / "state.json")


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

    def do_POST(self):
        try:
            length = int(self.headers.get("Content-Length", ""))
            if not 0 < length <= 262144:
                raise ValueError("Content-Length must be between 1 and 262144")
            request = json.loads(self.rfile.read(length))
            if not isinstance(request, dict):
                raise ValueError("request must be an object")
            if self.path == "/human":
                if set(request) != {"body", "status"} or not isinstance(request["body"], str):
                    raise ValueError("human edit must contain only a body string and a status")
                if request["status"] not in options:
                    raise ValueError("unknown human status")
                state["issues"]["work"].update(request)
                answer = {"data": {}}
            elif self.path == "/graphql":
                answer = self.graphql(request)
            else:
                raise ValueError("unknown route")
        except (ValueError, TypeError, KeyError) as error:
            self.reply({"errors": [{"message": str(error)}]}, 400)
            return
        save()
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


server = HTTPServer(("127.0.0.1", 0), Handler)
save()
(root / "endpoint").write_text(f"http://127.0.0.1:{server.server_port}")
server.serve_forever()
