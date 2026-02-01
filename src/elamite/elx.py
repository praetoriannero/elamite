from __future__ import annotations

from lark import Lark
from lark.tree import Meta
from pathlib import Path
from pprint import pprint
import sys

import elamite.elx_types as et
from elamite.analyzer import Analyzer
from elamite.elx_builtins import BUILTINS
from elamite.transformer import Builder
from elamite.options import CompilerOptions


ELS_PATTERN = "*.els"


class Elx:
    """
    Compile the code
    """

    def __init__(self, opts: CompilerOptions):
        self.opts = opts

        lines = None
        with open("elamite.lark", "r") as input_handle:
            lines = input_handle.read()

        self.parser = Lark(lines, propagate_positions=True, parser="lalr")
        self.builder = Builder()
        self.analyzer = Analyzer(self.opts.project_type)
        self.src_files: list[Path] = []
        self.program_ast = {}

    def discover(self) -> Elx:
        # TODO: find config.toml
        # TODO: discover all imports
        # TODO: find all source files
        self.src_files = [Path(node) for node in self.opts.input_path.glob(ELS_PATTERN)]

        for file_path in self.src_files:
            print(file_path)

        return self

    def parse(self) -> Elx:
        for file in self.src_files:
            with open(file, "r") as src_handle:
                mod_name = file.parts[-1].split(".")[0]
                src = src_handle.read()
                tree = self.parser.parse(src)
                ast = self.builder.transform(tree)
                ident = et.Identifier(mod_name)
                types = {
                    symbol.ident.name: symbol
                    for symbol in ast.values()
                    if isinstance(symbol, et.StructDef)
                }
                funcs = {
                    symbol.ident.name: symbol
                    for symbol in ast.values()
                    if isinstance(symbol, et.FuncDef)
                }
                globals = {
                    symbol.ident.name: symbol
                    for symbol in ast.values()
                    if isinstance(symbol, et.GlobalDef)
                }
                modules = {
                    symbol.ident.name: symbol
                    for symbol in ast.values()
                    if isinstance(symbol, et.Module)
                }
                imports = {
                    symbol.ident.name: symbol
                    for symbol in ast.values()
                    if isinstance(symbol, et.Import)
                }
                module = et.Module(
                    Meta(), ident, types, funcs, globals, modules, imports
                )
                self.program_ast.update({mod_name: module})

        pprint(self.program_ast)
        return self

    def analyze(self) -> Elx:
        self.analyzer.analyze(self.program_ast)
        return self

    def optimize(self) -> Elx:
        return self

    def transpile(self) -> Elx:
        return self

    def compile(self):
        pass


def main():
    opts = CompilerOptions(
        Path(sys.argv[1]),
    )
    compiler = Elx(opts)
    compiler.discover().parse().analyze().optimize().transpile().compile()


if __name__ == "__main__":
    main()
