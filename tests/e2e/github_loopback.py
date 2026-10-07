"""Loopback GitHub GraphQL for the reused-store write-back journey.

Every answer is built from current endpoint state. Unknown operations fail loudly;
no request reaches GitHub or reads a production credential.
"""
import json
import pathlib
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

root = pathlib.Path(sys.argv[1])
options = ["Todo", "Queued", "In Progress", "Needs Attention", "Done", "Canceled"]


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
                {"onepipeline.id": name, "onepipeline.persona": "engineer"})
    content = "" if name == "plan" else (
        f"## What\nDo {name}.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- {name} is done.")
    state["issues"][name] = {"body": described(content, metadata), "status": "Todo", "state": "OPEN",
                              "title": "github-edits" if name == "plan" else name}


def issue(name):
    held = state["issues"][name]
    membership = {"id": "ITEM-" + name, "project": {"id": "BOARD", "number": 1},
                  "fieldValues": connection([{"name": held["status"], "optionId": held["status"],
                                               "field": field()}])}
    value = {"__typename": "Issue", "id": name, "title": held["title"],
             "body": held["body"], "state": held["state"], "stateReason": None, "number": 1,
             "url": None, "parent": None if name == "plan" else {"id": "plan"},
             "subIssuesSummary": {"total": 3 if name == "plan" else 0},
             "labels": connection([]), "projectItems": connection([membership]),
             "blockedBy": connection([]), "blocking": connection([])}
    if name == "plan":
        value["subIssues"] = connection([issue("anchor"), issue("work"), issue("other")])
    return value


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
        if "updateProjectV2ItemFieldValue(" in query:
            supplied = variables.get("input")
            if (not isinstance(supplied, dict)
                    or set(supplied) != {"projectId", "itemId", "fieldId", "value"}
                    or supplied["projectId"] != "BOARD" or supplied["fieldId"] != "STATUS"
                    or supplied["itemId"] not in ["ITEM-" + name for name in state["issues"]]
                    or not isinstance(supplied["value"], dict)
                    or set(supplied["value"]) != {"singleSelectOptionId"}
                    or supplied["value"]["singleSelectOptionId"] not in options):
                raise ValueError("invalid Status mutation input")
            name = supplied["itemId"].removeprefix("ITEM-")
            state["issues"][name]["status"] = supplied["value"]["singleSelectOptionId"]
            return {"data": {"updateProjectV2ItemFieldValue": {"projectV2Item": {"id": supplied["itemId"]}}}}
        if "node(id:" in query:
            name = variables.get("id")
            if not isinstance(name, str) or name not in state["issues"]:
                raise ValueError("unknown issue id")
            return {"data": {"node": issue(name)}}
        if "repositoryOwner(" in query:
            board = {"id": "BOARD", "title": "Board", "fields": connection([field()]),
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
