from __future__ import annotations

from lark import Lark
from pathlib import Path
from pprint import pprint
import sys

from elamite.transformer import Builder
from elamite.options import CompilerOptions


class CLangTree:
    pass


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
                src = src_handle.read()
                tree = self.parser.parse(src)
                ast = self.builder.transform(tree)
                self.program_ast.update(ast)

        pprint(self.program_ast)
        return self

    def analyze(self) -> Elx:
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
