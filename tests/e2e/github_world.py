"""A loopback GitHub with more than one board and more than one repository, for the journeys
about where the linked store files an issue.

`tests/e2e/github_loopback.py` answers one fixed board by matching each request's text; this
one models what those requests are about — repositories with a visibility, Projects with a
`public` flag, the items on them and the issues the items hold — and answers every GraphQL
document by executing it against that model, so the store's own documents are answered as
GitHub answers them without this file restating any of them. A field the model does not hold
fails the request loudly, naming the type and the field, rather than answering it with
nothing.

GraphQL is served at `/board/<number>/graphql`, so the request log says which board's source
sent each request; every board reads the one model, as every GitHub endpoint does. `GET
/repos/<owner>/<name>` answers a repository's visibility over REST, as the store reads it
before filing an issue on a board declared private. `GET /state` answers the whole model and
the request log.

The first argument names the directory the URL is published to, whole, as `endpoint`; the
second a JSON file seeding the model: `{"owner": LOGIN, "repositories": {"owner/name":
"public" | "private"}, "boards": {"<number>": {"public": BOOL}}, "issues": [{"id": ID,
"repository": "owner/name", "title": T, "body": B, "board": NUMBER, "status": OPTION,
"parent": ID | null}]}`.
"""
import json
import os
import pathlib
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STATUS_OPTIONS = ["Backlog", "Todo", "Queued", "In Progress", "Needs Attention", "Done", "Canceled"]
ORIGIN_FIELD = "onetaskgraph.origin"
STAMP = "2026-10-01T00:00:00Z"


class Unsupported(Exception):
    """A request naming something this model does not hold."""


# ---------------------------------------------------------------------------------------------
# GraphQL: a parser and an executor for the subset of the language the store's documents use —
# operations with variables, aliases, arguments, inline fragments, named fragments and the
# `@include` and `@skip` directives.
# ---------------------------------------------------------------------------------------------

TOKEN = re.compile(r'''
    (?P<skip>[\s,]+|\#[^\n]*)
  | (?P<spread>\.\.\.)
  | (?P<string>"(?:[^"\\]|\\.)*")
  | (?P<number>-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)
  | (?P<name>[_A-Za-z][_0-9A-Za-z]*)
  | (?P<punct>[!$():=@\[\]{}|&])
''', re.VERBOSE)


def tokens(text):
    at, found = 0, []
    while at < len(text):
        matched = TOKEN.match(text, at)
        if not matched:
            raise Unsupported("unreadable GraphQL at %r" % text[at:at + 20])
        at = matched.end()
        if matched.lastgroup != "skip":
            found.append((matched.lastgroup, matched.group()))
    return found


class Parser:
    def __init__(self, text):
        self.tokens, self.at = tokens(text), 0

    def peek(self, value=None):
        if self.at >= len(self.tokens):
            return None if value is None else False
        token = self.tokens[self.at]
        return token if value is None else token[1] == value

    def take(self, value=None):
        token = self.peek()
        if token is None or (value is not None and token[1] != value):
            raise Unsupported("expected %r, found %r" % (value, token))
        self.at += 1
        return token[1]

    def document(self):
        operations, fragments = [], {}
        while self.peek() is not None:
            if self.peek("fragment"):
                self.take()
                name = self.take()
                self.take("on")
                condition = self.take()
                self.directives()
                fragments[name] = (condition, self.selections())
            elif self.peek("{"):
                operations.append(("query", self.selections()))
            else:
                kind = self.take()
                if not self.peek("(") and not self.peek("{") and not self.peek("@"):
                    self.take()
                if self.peek("("):
                    self.variable_definitions()
                self.directives()
                operations.append((kind, self.selections()))
        if len(operations) != 1:
            raise Unsupported("a document must hold exactly one operation")
        return operations[0], fragments

    def variable_definitions(self):
        self.take("(")
        while not self.peek(")"):
            self.take("$")
            self.take()
            self.take(":")
            self.type()
            if self.peek("="):
                self.take()
                self.value()
        self.take(")")

    def type(self):
        if self.peek("["):
            self.take()
            self.type()
            self.take("]")
        else:
            self.take()
        if self.peek("!"):
            self.take()

    def directives(self):
        found = []
        while self.peek("@"):
            self.take()
            found.append((self.take(), self.arguments()))
        return found

    def arguments(self):
        found = {}
        if self.peek("("):
            self.take()
            while not self.peek(")"):
                name = self.take()
                self.take(":")
                found[name] = self.value()
            self.take(")")
        return found

    def value(self):
        kind, text = self.peek()
        if text == "$":
            self.take()
            return ("var", self.take())
        if text == "[":
            self.take()
            items = []
            while not self.peek("]"):
                items.append(self.value())
            self.take("]")
            return ("list", items)
        if text == "{":
            self.take()
            fields = {}
            while not self.peek("}"):
                name = self.take()
                self.take(":")
                fields[name] = self.value()
            self.take("}")
            return ("object", fields)
        self.take()
        if kind == "string":
            return ("const", json.loads(text))
        if kind == "number":
            return ("const", json.loads(text))
        return ("const", {"true": True, "false": False, "null": None}.get(text, text))

    def selections(self):
        self.take("{")
        found = []
        while not self.peek("}"):
            if self.peek("..."):
                self.take()
                if self.peek("on"):
                    self.take()
                    condition = self.take()
                    directives = self.directives()
                    found.append(("inline", condition, directives, self.selections()))
                elif self.peek("{") or self.peek("@"):
                    directives = self.directives()
                    found.append(("inline", None, directives, self.selections()))
                else:
                    name = self.take()
                    found.append(("spread", name, self.directives()))
            else:
                alias = name = self.take()
                if self.peek(":"):
                    self.take()
                    name = self.take()
                arguments = self.arguments()
                directives = self.directives()
                nested = self.selections() if self.peek("{") else None
                found.append(("field", alias, name, arguments, directives, nested))
        self.take("}")
        return found


def evaluate(value, variables):
    kind, held = value
    if kind == "var":
        return variables.get(held)
    if kind == "list":
        return [evaluate(item, variables) for item in held]
    if kind == "object":
        return {name: evaluate(item, variables) for name, item in held.items()}
    return held


def included(directives, variables):
    for name, arguments in directives:
        condition = evaluate(arguments.get("if", ("const", True)), variables)
        if name == "include" and not condition:
            return False
        if name == "skip" and condition:
            return False
    return True


def merge(into, value):
    for key, held in value.items():
        if key in into and isinstance(into[key], dict) and isinstance(held, dict):
            merge(into[key], held)
        else:
            into[key] = held


class Execution:
    def __init__(self, fragments, variables):
        self.fragments, self.variables = fragments, variables

    def select(self, target, selections):
        answer = {}
        for selection in selections:
            if selection[0] == "field":
                _, alias, name, arguments, directives, nested = selection
                if not included(directives, self.variables):
                    continue
                if name == "__typename":
                    merge(answer, {alias: target.typename})
                    continue
                supplied = {key: evaluate(value, self.variables)
                            for key, value in arguments.items()}
                merge(answer, {alias: self.shape(target.field(name, supplied), nested)})
            elif selection[0] == "inline":
                _, condition, directives, nested = selection
                if included(directives, self.variables) and target.is_a(condition):
                    merge(answer, self.select(target, nested))
            else:
                _, name, directives = selection
                condition, nested = self.fragments[name]
                if included(directives, self.variables) and target.is_a(condition):
                    merge(answer, self.select(target, nested))
        return answer

    def shape(self, value, nested):
        if isinstance(value, list):
            return [self.shape(item, nested) for item in value]
        if isinstance(value, Node):
            if nested is None:
                raise Unsupported("%s needs a selection" % value.typename)
            return self.select(value, nested)
        return value


# ---------------------------------------------------------------------------------------------
# The model.
# ---------------------------------------------------------------------------------------------

class Node:
    typename = "Node"
    kinds = ()

    def is_a(self, condition):
        return condition is None or condition == self.typename or condition in self.kinds

    def field(self, name, arguments):
        resolver = getattr(self, "f_" + name, None)
        if resolver is None:
            raise Unsupported("the loopback models no %s.%s" % (self.typename, name))
        return resolver(arguments)


class Plain(Node):
    """An object answered field by field from a dict, for small result shapes."""

    def __init__(self, typename, fields, kinds=()):
        self.typename, self.fields, self.kinds = typename, fields, kinds

    def field(self, name, arguments):
        if name not in self.fields:
            raise Unsupported("the loopback models no %s.%s" % (self.typename, name))
        held = self.fields[name]
        return held(arguments) if callable(held) else held


def connection(nodes, arguments=None):
    first = (arguments or {}).get("first")
    shown = nodes if first is None else nodes[:first]
    return Plain("Connection", {
        "nodes": shown, "totalCount": len(nodes),
        "pageInfo": Plain("PageInfo", {"hasNextPage": len(shown) < len(nodes),
                                       "endCursor": None}),
    })


class World:
    def __init__(self, seed):
        self.owner = seed["owner"]
        self.repositories = {}
        for slug, visibility in seed["repositories"].items():
            if visibility not in ("public", "private"):
                raise SystemExit("a repository is public or private")
            self.repositories[slug] = {"id": "R_" + slug.replace("/", "_"),
                                       "visibility": visibility, "issues": 0}
        self.boards = {}
        for number, board in seed["boards"].items():
            self.boards[int(number)] = {
                "id": "PVT_%s" % number, "public": bool(board["public"]), "items": [],
                "fields": [
                    {"id": "STATUS_%s" % number, "name": "Status", "kind": "select"},
                    {"id": "ORIGIN_%s" % number, "name": ORIGIN_FIELD, "kind": "text"},
                    {"id": "TITLE_%s" % number, "name": "Title", "kind": "title"},
                ],
            }
        self.issues, self.items, self.minted = {}, {}, 0
        for issue in seed["issues"]:
            self.create_issue(issue["repository"], issue["title"], issue["body"],
                              issue.get("parent"), issue["id"])
            item = self.add_item(issue["board"], issue["id"])
            self.items[item]["values"]["STATUS_%s" % issue["board"]] = issue["status"]

    def mint(self, prefix):
        self.minted += 1
        return "%s_%d" % (prefix, self.minted)

    def repository_by_id(self, node):
        for slug, held in self.repositories.items():
            if held["id"] == node:
                return slug
        raise Unsupported("no repository has id %r" % node)

    def create_issue(self, repository, title, body, parent, node=None):
        if repository not in self.repositories:
            raise Unsupported("no repository %r" % repository)
        held = self.repositories[repository]
        held["issues"] += 1
        node = node or self.mint("I")
        self.issues[node] = {"id": node, "repository": repository, "number": held["issues"],
                             "title": title, "body": body, "state": "OPEN",
                             "stateReason": None, "parent": None, "subIssues": [],
                             "blockedBy": []}
        if parent is not None:
            self.add_sub_issue(parent, node)
        return node

    def add_sub_issue(self, parent, child):
        if parent not in self.issues or child not in self.issues:
            raise Unsupported("a sub-issue link names an unknown issue")
        self.issues[child]["parent"] = parent
        if child not in self.issues[parent]["subIssues"]:
            self.issues[parent]["subIssues"].append(child)

    def add_item(self, number, content):
        board = self.boards.get(number)
        if board is None or content not in self.issues:
            raise Unsupported("an item names an unknown board or issue")
        for item in board["items"]:
            if self.items[item]["content"] == content:
                return item
        item = self.mint("PVTI")
        self.items[item] = {"id": item, "board": number, "content": content, "values": {}}
        board["items"].append(item)
        return item

    def board_by_id(self, node):
        for number, board in self.boards.items():
            if board["id"] == node:
                return number
        raise Unsupported("no board has id %r" % node)

    def snapshot(self):
        return {"repositories": self.repositories, "boards": self.boards,
                "issues": self.issues, "items": self.items}


class Owner(Node):
    typename = "Organization"
    kinds = ("RepositoryOwner", "ProjectV2Owner", "Node")

    def __init__(self, world):
        self.world = world

    def f_login(self, _):
        return self.world.owner

    def f_projectV2(self, arguments):
        number = arguments["number"]
        return Board(self.world, number) if number in self.world.boards else None


class Board(Node):
    typename = "ProjectV2"
    kinds = ("Node",)

    def __init__(self, world, number):
        self.world, self.number = world, number
        self.held = world.boards[number]

    def f_id(self, _):
        return self.held["id"]

    def f_number(self, _):
        return self.number

    def f_title(self, _):
        return "Board %d" % self.number

    def f_public(self, _):
        return self.held["public"]

    def f_url(self, _):
        return "https://github.com/orgs/%s/projects/%d" % (self.world.owner, self.number)

    def f_fields(self, arguments):
        return connection([Field(self.number, held) for held in self.held["fields"]], arguments)

    def f_field(self, arguments):
        for held in self.held["fields"]:
            if held["name"] == arguments["name"]:
                return Field(self.number, held)
        return None

    def f_items(self, arguments):
        items = [Item(self.world, item) for item in self.held["items"]]
        query = arguments.get("query")
        if query:
            matched = re.fullmatch(r'(\S+):"((?:[^"\\]|\\.)*)"', query.strip())
            if not matched:
                raise Unsupported("the loopback filters items by one text field: %r" % query)
            field, wanted = matched.group(1), matched.group(2).replace('\\"', '"')
            items = [item for item in items if item.text(field) == wanted]
        return connection(items, arguments)


class Field(Node):
    kinds = ("ProjectV2FieldConfiguration", "ProjectV2FieldCommon", "Node")

    def __init__(self, number, held):
        self.number, self.held = number, held
        self.typename = ("ProjectV2SingleSelectField" if held["kind"] == "select"
                         else "ProjectV2Field")

    def f_id(self, _):
        return self.held["id"]

    def f_name(self, _):
        return self.held["name"]

    def f_dataType(self, _):
        return {"select": "SINGLE_SELECT", "text": "TEXT", "title": "TITLE"}[self.held["kind"]]

    def f_options(self, _):
        if self.held["kind"] != "select":
            raise Unsupported("a text field has no options")
        return [Plain("ProjectV2SingleSelectFieldOption",
                      {"id": "%s_%s" % (self.held["id"], option), "name": option,
                       "color": "GRAY", "description": ""})
                for option in STATUS_OPTIONS]


class Item(Node):
    typename = "ProjectV2Item"
    kinds = ("Node",)

    def __init__(self, world, item):
        self.world, self.held = world, world.items[item]

    def board(self):
        return self.world.boards[self.held["board"]]

    def text(self, name):
        for field in self.board()["fields"]:
            if field["name"] == name:
                return self.held["values"].get(field["id"])
        return None

    def values(self):
        found = []
        for field in self.board()["fields"]:
            value = self.held["values"].get(field["id"])
            if value is None:
                continue
            if field["kind"] == "select":
                found.append(Plain("ProjectV2ItemFieldSingleSelectValue", {
                    "name": value, "optionId": "%s_%s" % (field["id"], value),
                    "field": Field(self.held["board"], field)},
                    ("ProjectV2ItemFieldValue",)))
            else:
                found.append(Plain("ProjectV2ItemFieldTextValue", {
                    "text": value, "field": Field(self.held["board"], field)},
                    ("ProjectV2ItemFieldValue",)))
        return found

    def f_id(self, _):
        return self.held["id"]

    def f_updatedAt(self, _):
        return STAMP

    def f_createdAt(self, _):
        return STAMP

    def f_isArchived(self, _):
        return False

    def f_type(self, _):
        return "ISSUE"

    def f_project(self, _):
        return Board(self.world, self.held["board"])

    def f_content(self, _):
        return Issue(self.world, self.held["content"])

    def f_fieldValues(self, arguments):
        return connection(self.values(), arguments)

    def f_fieldValueByName(self, arguments):
        for value in self.values():
            if value.fields["field"].held["name"] == arguments["name"]:
                return value
        return None


class Issue(Node):
    typename = "Issue"
    kinds = ("Node", "ProjectV2ItemContent", "Closable", "Assignable", "Labelable",
             "Comment", "UniformResourceLocatable")

    def __init__(self, world, node):
        self.world, self.held = world, world.issues[node]

    def f_id(self, _):
        return self.held["id"]

    def f_number(self, _):
        return self.held["number"]

    def f_title(self, _):
        return self.held["title"]

    def f_body(self, _):
        return self.held["body"]

    def f_url(self, _):
        return "https://github.com/%s/issues/%d" % (self.held["repository"], self.held["number"])

    def f_createdAt(self, _):
        return STAMP

    def f_updatedAt(self, _):
        return STAMP

    def f_closed(self, _):
        return self.held["state"] == "CLOSED"

    def f_state(self, _):
        return self.held["state"]

    def f_stateReason(self, _):
        return self.held["stateReason"]

    def f_repository(self, _):
        return Repository(self.world, self.held["repository"])

    def f_parent(self, _):
        parent = self.held["parent"]
        return None if parent is None else Issue(self.world, parent)

    def f_subIssues(self, arguments):
        return connection([Issue(self.world, child) for child in self.held["subIssues"]],
                          arguments)

    def f_subIssuesSummary(self, _):
        children = self.held["subIssues"]
        done = sum(1 for child in children if self.world.issues[child]["state"] == "CLOSED")
        return Plain("SubIssuesSummary", {"total": len(children), "completed": done,
                                          "percentCompleted": 0})

    def f_comments(self, arguments):
        return connection([], arguments)

    def f_timelineItems(self, arguments):
        return connection([], arguments)

    def f_labels(self, arguments):
        return connection([], arguments)

    def f_assignees(self, arguments):
        return connection([], arguments)

    def f_projectItems(self, arguments):
        held = [Item(self.world, item) for item, value in self.world.items.items()
                if value["content"] == self.held["id"]]
        return connection(held, arguments)

    def f_blockedBy(self, arguments):
        return connection([Issue(self.world, by) for by in self.held["blockedBy"]], arguments)

    def f_blocking(self, arguments):
        blocking = [node for node, held in self.world.issues.items()
                    if self.held["id"] in held["blockedBy"]]
        return connection([Issue(self.world, node) for node in blocking], arguments)


class Repository(Node):
    typename = "Repository"
    kinds = ("Node",)

    def __init__(self, world, slug):
        self.world, self.slug = world, slug
        self.held = world.repositories[slug]

    def f_id(self, _):
        return self.held["id"]

    def f_databaseId(self, _):
        return sorted(self.world.repositories).index(self.slug) + 1

    def f_nameWithOwner(self, _):
        return self.slug

    def f_name(self, _):
        return self.slug.split("/")[1]

    def f_owner(self, _):
        return Owner(self.world)

    def f_visibility(self, _):
        return self.held["visibility"].upper()

    def f_isPrivate(self, _):
        return self.held["visibility"] == "private"

    def f_issue(self, arguments):
        for node, held in self.world.issues.items():
            if held["repository"] == self.slug and held["number"] == arguments["number"]:
                return Issue(self.world, node)
        return None


class Query(Node):
    typename = "Query"

    def __init__(self, world):
        self.world = world

    def f_repositoryOwner(self, arguments):
        return Owner(self.world) if arguments["login"] == self.world.owner else None

    def f_organization(self, arguments):
        return self.f_repositoryOwner(arguments)

    def f_repository(self, arguments):
        slug = "%s/%s" % (arguments["owner"], arguments["name"])
        return Repository(self.world, slug) if slug in self.world.repositories else None

    def f_node(self, arguments):
        node = arguments["id"]
        if node in self.world.issues:
            return Issue(self.world, node)
        if node in self.world.items:
            return Item(self.world, node)
        for number, board in self.world.boards.items():
            if board["id"] == node:
                return Board(self.world, number)
        return None

    def f_nodes(self, arguments):
        return [self.f_node({"id": node}) for node in arguments["ids"]]

    def f_search(self, arguments):
        """GitHub's issue search, over the qualifiers the store scopes it with: `project:`,
        `is:issue`, `in:title` and `in:body`, and quoted phrases, each of which must be held."""
        if arguments["type"] != "ISSUE":
            raise Unsupported("the loopback searches issues only")
        terms = re.findall(r'"(?:[^"\\]|\\.)*"|\S+', arguments["query"])
        board, fields, phrases = None, set(), []
        for term in terms:
            if term.startswith('"'):
                phrases.append(json.loads(term))
            elif term.startswith("project:"):
                owner, number = term[len("project:"):].split("/")
                if owner != self.world.owner:
                    return connection([], arguments)
                board = int(number)
            elif term.startswith("in:"):
                fields.update(term[len("in:"):].split(","))
            elif term != "is:issue" and not term.startswith("updated:"):
                raise Unsupported("the loopback searches with no %r" % term)
        fields = fields or {"title", "body"}
        found = []
        for node, held in self.world.issues.items():
            if board is not None and not any(
                    item["board"] == board and item["content"] == node
                    for item in self.world.items.values()):
                continue
            if all(any(phrase in held[field] for field in fields) for phrase in phrases):
                found.append(Issue(self.world, node))
        return connection(found, arguments)

    def f_rateLimit(self, _):
        return Plain("RateLimit", {"cost": 1, "remaining": 4999, "limit": 5000,
                                   "resetAt": STAMP, "used": 1, "nodeCount": 1})


class Mutation(Node):
    typename = "Mutation"

    def __init__(self, world):
        self.world = world

    def f_createIssue(self, arguments):
        supplied = arguments["input"]
        repository = self.world.repository_by_id(supplied["repositoryId"])
        node = self.world.create_issue(repository, supplied["title"], supplied.get("body", ""),
                                       supplied.get("parentIssueId"))
        for board in supplied.get("projectV2Ids") or []:
            self.world.add_item(self.world.board_by_id(board), node)
        return Plain("CreateIssuePayload", {"issue": Issue(self.world, node)})

    def f_addProjectV2ItemById(self, arguments):
        supplied = arguments["input"]
        item = self.world.add_item(self.world.board_by_id(supplied["projectId"]),
                                   supplied["contentId"])
        return Plain("AddProjectV2ItemByIdPayload", {"item": Item(self.world, item)})

    def f_updateIssue(self, arguments):
        supplied = arguments["input"]
        held = self.world.issues.get(supplied["id"])
        if held is None:
            raise Unsupported("updateIssue names an unknown issue")
        for key in ("title", "body"):
            if key in supplied:
                held[key] = supplied[key]
        movement = supplied.get("stateInput") or {}
        if "state" in supplied:
            movement = {"value": supplied["state"], "stateReason": supplied.get("stateReason")}
        if movement:
            held["state"] = movement["value"]
            held["stateReason"] = movement.get("stateReason")
        return Plain("UpdateIssuePayload", {"issue": Issue(self.world, supplied["id"])})

    def write(self, supplied, clear):
        item = self.world.items.get(supplied["itemId"])
        if item is None or self.world.boards[item["board"]]["id"] != supplied["projectId"]:
            raise Unsupported("a field write names an item not on its board")
        field = next((field for field in self.world.boards[item["board"]]["fields"]
                      if field["id"] == supplied["fieldId"]), None)
        if field is None:
            raise Unsupported("a field write names an unknown field")
        if clear:
            item["values"].pop(field["id"], None)
        elif field["kind"] == "select":
            option = supplied["value"]["singleSelectOptionId"]
            prefix = field["id"] + "_"
            if not option.startswith(prefix) or option[len(prefix):] not in STATUS_OPTIONS:
                raise Unsupported("an unknown Status option %r" % option)
            item["values"][field["id"]] = option[len(prefix):]
        else:
            item["values"][field["id"]] = supplied["value"]["text"]
        return Plain("Payload", {"projectV2Item": Item(self.world, item["id"])})

    def f_updateProjectV2ItemFieldValue(self, arguments):
        return self.write(arguments["input"], False)

    def f_clearProjectV2ItemFieldValue(self, arguments):
        return self.write(arguments["input"], True)

    def f_addSubIssue(self, arguments):
        supplied = arguments["input"]
        self.world.add_sub_issue(supplied["issueId"], supplied["subIssueId"])
        return Plain("AddSubIssuePayload", {"issue": Issue(self.world, supplied["issueId"]),
                                            "subIssue": Issue(self.world, supplied["subIssueId"])})

    def f_addBlockedBy(self, arguments):
        supplied = arguments["input"]
        held = self.world.issues[supplied["issueId"]]
        if supplied["blockingIssueId"] not in held["blockedBy"]:
            held["blockedBy"].append(supplied["blockingIssueId"])
        return Plain("AddBlockedByPayload", {
            "issue": Issue(self.world, supplied["issueId"]),
            "blockingIssue": Issue(self.world, supplied["blockingIssueId"])})


# ---------------------------------------------------------------------------------------------
# The server.
# ---------------------------------------------------------------------------------------------

world = World(json.loads(pathlib.Path(sys.argv[2]).read_text()))
requests = []
unsupported = []
lock = threading.Lock()


def answer_graphql(path, request):
    query, variables = request.get("query"), request.get("variables") or {}
    if not isinstance(query, str) or not isinstance(variables, dict):
        raise Unsupported("GraphQL requires a query string and an object of variables")
    requests.append({"path": path, "query": query, "variables": variables})
    (kind, selections), fragments = Parser(query).document()
    root = Mutation(world) if kind == "mutation" else Query(world)
    return {"data": Execution(fragments, variables).select(root, selections)}


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
        with lock:
            if self.path == "/state":
                self.reply({**world.snapshot(), "requests": requests,
                            "unsupported": unsupported})
                return
            matched = re.fullmatch(r"/repos/([^/]+)/([^/?]+)", self.path)
            if matched:
                slug = "%s/%s" % matched.groups()
                requests.append({"path": self.path, "method": "GET"})
                held = world.repositories.get(slug)
                if held is None:
                    self.reply({"message": "Not Found"}, 404)
                    return
                self.reply({"id": 1, "node_id": held["id"], "full_name": slug,
                            "private": held["visibility"] == "private",
                            "visibility": held["visibility"]})
                return
        self.reply({"message": "Not Found"}, 404)

    def do_POST(self):
        if not re.fullmatch(r"/board/\d+/graphql", self.path):
            self.reply({"message": "Not Found"}, 404)
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            request = json.loads(self.rfile.read(length))
            with lock:
                answer = answer_graphql(self.path, request)
        except (Unsupported, KeyError, TypeError, ValueError) as error:
            with lock:
                unsupported.append(str(error))
            print("github_world: %s" % error, file=sys.stderr, flush=True)
            self.reply({"errors": [{"message": "loopback: %s" % error}]}, 400)
            return
        self.reply(answer)


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
root = pathlib.Path(sys.argv[1])
staged = root / "endpoint.partial"
staged.write_text("http://127.0.0.1:%d" % server.server_port)
os.replace(staged, root / "endpoint")
server.serve_forever()
