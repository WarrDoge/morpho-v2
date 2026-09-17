import ast
import json
import pathlib

code = [p for p in pathlib.Path(".").rglob("*.py") if not p.name.startswith("_") and "tests" not in p.parts]
tests = [p for p in pathlib.Path("tests").rglob("*.py")] if pathlib.Path("tests").is_dir() else []
funcs, imports_re, raises, lines = [], False, 0, 0
for path in code:
    try:
        tree = ast.parse(path.read_text())
    except SyntaxError:
        continue
    lines += len([l for l in path.read_text().splitlines() if l.strip() and not l.strip().startswith("#")])
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            funcs.append(node)
        elif isinstance(node, ast.Import) and any(a.name == "re" for a in node.names):
            imports_re = True
        elif isinstance(node, ast.ImportFrom) and node.module == "re":
            imports_re = True
        elif isinstance(node, ast.Raise):
            raises += 1
annotated = [f for f in funcs if f.returns or any(a.annotation for a in f.args.args)]
documented = [f for f in funcs if ast.get_docstring(f)]
test_functions = sum(t.read_text().count("def test_") for t in tests)
share = lambda xs: round(len(xs) / len(funcs), 4) if funcs else None
print(json.dumps({"uses_re": imports_re, "functions": len(funcs), "annotated": share(annotated),
                  "documented": share(documented), "raises": raises, "code_lines": lines,
                  "test_functions": test_functions}))
