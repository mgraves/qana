// A MINI language from scratch — proof that the playground serves any
// envelope grammar, not just the demo. This one is "Tasklang": task
// declarations with properties, braces, comments, and strings.
//
// Open demo.cl next to this file (F5 the extension, open this folder).
// Edit ANYTHING here and save: the pipeline re-certifies and open
// documents re-colorize. This file itself gets rantlr highlighting,
// outline, and live envelope diagnostics as you type.

language Tasklang

token WS = /\s+/ @trivia
token LINE_COMMENT = /#.*/ @trivia @style(comment)
token STRING = /"(\\.|[^"\\])*"/ @style(string)
token NUMBER = /\d+/ @style(number)
token IDENT = /[\a_][\w_]*/ @specialize @style(variable)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token SEMI = ";" @style(punctuation)

keywords IDENT = task due tag done blocked by

pair LBRACE RBRACE

start file

rule file = File: task_list

rule task_list = task_def*

rule task_def = TaskDef: "task" name:STRING body @outline(name)

rule body = Body: "{" prop* "}" @scope

rule prop =
  | Due: "due" NUMBER ";"
  | Tag: "tag" IDENT ";"
  | Done: "done" ";"
  | Blocked: "blocked" "by" STRING ";"
