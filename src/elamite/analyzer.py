from enum import Enum

import elamite.elx_types as et
from elamite.errors import MainNotFound


class ProjectType(Enum):
    BIN = 0
    LIB = 1


class Analyzer:
    """
    Performs the following on the AST:
        - type checking between LHS and RHS
        - type inference on LHS
        - correct initialization
        - control flow validation
        - dead code elimination
        - constant folding
    """

    def __init__(self, ast: dict, bin_type: ProjectType) -> None:
        self.ast = ast
        self.bin_type = bin_type

        self.scope = {}
        self.local = {}
        self.main_exists = False

        if self.bin_type == ProjectType.BIN:
            self._verify_main()

    def _verify_main(self):
        if "main" in self.ast:
            symbol = self.ast["main"]
            if isinstance(symbol, et.FuncDef):
                self.main_exists = True
                return

        raise MainNotFound(
            f"elx missing entry point 'fn main' for project type {self.bin_type}"
        )

    def visit_symbol(self, ident: et.Identifier, item: et.Item):
        if isinstance(item, et.FuncDef):
            self.visit_func(item)

    def visit_module(self, module: et.Module):
        raise NotImplementedError()

    def visit_global(self, global_: et.GlobalDef):
        raise NotImplementedError()

    def visit_enum(self, enum_: et.EnumDef):
        raise NotImplementedError()

    def visit_block(self, block: list[et.Stmt]):
        raise NotImplementedError()

    def visit_func(self, func: et.FuncDef):
        raise NotImplementedError()

    def visit_assign_stmt(self, stmt: et.AssignStmt):
        raise NotImplementedError()

    def visit_let_stmt(self, stmt: et.LetStmt):
        raise NotImplementedError()

    def visit_return_stmt(self, stmt: et.ReturnStmt):
        raise NotImplementedError()

    def visit_expr_stmt(self, stmt: et.ExprStmt):
        raise NotImplementedError()

    def visit_for_stmt(self, stmt: et.ForStmt):
        raise NotImplementedError()

    def visit_while_stmt(self, stmt: et.WhileStmt):
        raise NotImplementedError()

    def visit_if_stmt(self, stmt: et.IfStmt):
        raise NotImplementedError()

    def visit_break_stmt(self, stmt: et.BreakStmt):
        raise NotImplementedError()

    def visit_continue_stmt(self, stmt: et.ContinueStmt):
        raise NotImplementedError()

    def visit_module_stmt(self, stmt: et.ModuleStmt):
        raise NotImplementedError()

    def analyze(self) -> dict:
        for ident, symbol in self.ast.items():
            self.visit_symbol(ident, symbol)

        return self.ast
