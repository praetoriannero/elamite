from typing import cast

import elamite.elx_types as et
from elamite.elx_builtins import BUILTINS
from elamite.errors import TypeError
from elamite.options import ProjectType


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

    def __init__(self, bin_type: ProjectType) -> None:
        self.bin_type = bin_type

        self.ast = {}
        self.scope = []
        self.symbols = {}
        self.vars = {}
        self.main_exists = False
        self.dispatch = {
            et.Module: self.visit_module,
            et.StructDef: self.visit_struct_def,
            et.FuncDef: self.visit_func,
            et.GlobalDef: self.visit_global,
            et.EnumDef: self.visit_enum,
            et.Import: self.visit_import,
            et.AssignStmt: self.visit_assign_stmt,
            et.LetStmt: self.visit_let_stmt,
            et.ReturnStmt: self.visit_return_stmt,
            et.ExprStmt: self.visit_expr_stmt,
            et.ForStmt: self.visit_for_stmt,
            et.WhileStmt: self.visit_while_stmt,
            et.IfStmt: self.visit_if_stmt,
            et.BreakStmt: self.visit_break_stmt,
            et.ContinueStmt: self.visit_continue_stmt,
            et.Module: self.visit_module,
        }

    def _verify_main(self):
        pass
        # if "main" in self.ast:
        #     symbol = self.ast["main"]
        #     if isinstance(symbol, et.FuncDef):
        #         self.main_exists = True
        #         return
        #
        # raise MainNotFound(
        #     f"elx missing entry point function 'main' for project type {self.bin_type}"
        # )

    def _resolve_type(self, type: et.Type | None):
        if type is None:
            return False

        if type.ident in BUILTINS:
            return True

        mods = self.ast
        for ns in self.scope:
            types = mods[ns].types
            if type.ident.name in types:
                return True

            mods = mods[ns].mods

        return False

    def _resolve_expr(self, expr) -> et.Type | None:
        match expr:
            case et.Integer():
                return et.Type(et.Identifier("u64"))
            case et.Float():
                return et.Type(et.Identifier("f64"))
            case et.BinaryOp():
                return None

        return None

    def visit_module(self, module: et.Module):
        self.scope.append(module.ident.name)
        self.symbols[tuple(self.scope)] = {}

        for attr in ("types", "funcs", "globals", "mods", "imports"):
            for name, symbol in getattr(module, attr).items():
                self.dispatch[type(symbol)](symbol)  # type: ignore

        self.scope.pop()

    def visit_struct_def(self, struct_def: et.StructDef):
        for field in struct_def.fields:
            if not self._resolve_type(field.type):
                raise TypeError(
                    f"In module '{self.scope[-1]}': "
                    f"Undefined type '{field.type.ident.name}' found in struct "
                    f"'{struct_def.ident.name}' on line {struct_def.meta.line}"
                )

    def visit_global(self, global_: et.GlobalDef):
        if not self._resolve_type(global_.type):
            raise TypeError(
                f"In module '{self.scope[-1]}': "
                f"Undefined type '{global_.type.ident.name}' found on line {global_.meta.line}"
            )

    def visit_func(self, func: et.FuncDef):
        for param in func.params:
            if not self._resolve_type(param.type):
                raise TypeError(
                    f"In module '{self.scope[-1]}': "
                    f"Undefined type '{param.type.ident.name}' for function "
                    f"'{func.ident.name}' parameter '{param.ident.name}' on line {func.meta.line}"
                )

        if func.ret_type is None:
            func.ret_type = et.Unit()

        if not self._resolve_type(func.ret_type):
            raise TypeError(
                f"In module '{self.scope[-1]}': "
                f"Undefined return type '{func.ret_type.ident.name}' for "
                f"function '{func.ident.name}' on line {func.meta.line}"
            )

        self.visit_block(func.block)

    def visit_enum(self, enum_: et.EnumDef):
        raise NotImplementedError()

    def visit_import(self, import_: et.Import):
        raise NotImplementedError()

    def visit_block(self, block: list[et.Stmt]):
        block_vars = {}
        for stmt in block:
            if isinstance(stmt, et.LetStmt):
                block_vars[stmt.ident.name] = stmt
                self.vars.update(block_vars)

            self.dispatch[type(stmt)](stmt)  # type: ignore

        for var in block_vars.keys():
            self.vars.pop(var)

    def visit_assign_stmt(self, stmt: et.AssignStmt):
        raise NotImplementedError()

    def visit_let_stmt(self, stmt: et.LetStmt):
        type_ = stmt.type
        if type_ is None:
            type_ = self._resolve_expr(stmt.expr)

        if not self._resolve_type(type_):
            if type_ is None:
                raise TypeError(
                    f"In module '{self.scope[-1]}': "
                    f"Unable to resolve type for variable '{stmt.ident.name}'"
                    f" on line {stmt.meta.line}"
                )

            raise TypeError(
                f"In module '{self.scope[-1]}': "
                f"Undefined type '{type_.ident.name}' for "
                f"right-hand expression '{type_.ident.name}' on line {stmt.meta.line}"
            )

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

    def analyze(self, ast: dict) -> None:
        self.ast = ast

        for ident, item in ast.items():
            self.dispatch[type(item)](item)

        if self.bin_type == ProjectType.BIN:
            self._verify_main()
