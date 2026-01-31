from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from lark import Lark, Transformer, v_args
from pathlib import Path
from pprint import pprint
import sys


from elamite.types import (
    AssignStmt,
    BinaryOp,
    BinOp,
    Bool,
    Float,
    Identifier,
    Integer,
    ExprStmt,
    ModuleStmt,
    Range,
    RangeEnd,
    RangeInclusive,
    RangeStart,
)


class Builder(Transformer):
    NULL = str
    MUT = str

    def ident(self, ident):
        return Identifier(str(ident[0]))

    def integer(self, integer):
        value = str(integer[0])
        return Integer(value)

    def float(self, float):
        value = str(float[0])
        return Float(value)

    def mut(self, mut):
        return "mut"

    def bool_true(self, atom):
        return Bool(True)

    def bool_false(self, atom):
        return Bool(False)

    def stmt(self, stmt):
        return stmt[0]

    # namespacing
    @v_args(meta=True)
    def module_stmt(self, meta, module):
        ident = module[0]
        block = module[1:]
        return ModuleStmt(meta, ident, block[0])

    # block statements
    @v_args(meta=True)
    def expr_stmt(self, meta, stmt):
        return ExprStmt(meta, *stmt)

    @v_args(meta=True)
    def assign_stmt(self, meta, assign_stmt):
        lhs, op, rhs = assign_stmt
        return AssignStmt(meta, op, lhs, rhs)

    @v_args(meta=True)
    def let_stmt(self, meta, stmt):
        idx = 0
        mut = False
        if "mut" == stmt[idx]:
            mut = True
            idx += 1

        ident = stmt[idx]
        idx += 1

        if isinstance(stmt[idx], Type):
            type = stmt[idx]
            idx += 1
            expr = stmt[idx]
        else:
            expr = stmt[idx]
            type = None  # determined during analysis

        return LetStmt(meta, ident, type, expr, mut)

    @v_args(meta=True)
    def for_stmt(self, meta, stmt):
        return ForStmt(meta, stmt[0], stmt[1], stmt[2])

    # conditionals
    @v_args(meta=True)
    def while_stmt(self, meta, stmt):
        return WhileStmt(meta, stmt[0], stmt[1])

    @v_args(meta=True)
    def if_stmt(self, meta, stmt):
        expr = stmt[0]
        block = stmt[1]
        elif_clauses = [clause for clause in stmt if isinstance(clause, ElifClause)]
        if isinstance(stmt[-1], ElseClause):
            else_clause = stmt[-1]
        else:
            else_clause = None

        return IfStmt(meta, expr, block, elif_clauses, else_clause)

    @v_args(meta=True)
    def elif_clause(self, meta, clause):
        return ElifClause(meta, clause[0], clause[1])

    @v_args(meta=True)
    def else_clause(self, meta, clause):
        return ElseClause(meta, clause[0])

    # control flow
    @v_args(meta=True)
    def break_stmt(self, meta, stmt):
        # TODO: handle ident case
        return BreakStmt(meta, None)

    @v_args(meta=True)
    def continue_stmt(self, meta, stmt):
        # TODO: handle ident case
        return ContinueStmt(meta, None)

    @v_args(meta=True)
    def return_stmt(self, meta, stmt):
        if len(stmt):
            expr = stmt[0]
        else:
            expr = None

        return ReturnStmt(meta, expr)

    def start(self, module):
        return module[0]

    def module(self, symbols):
        # TODO: handle case where a symbol is defined multiple times
        if not symbols:
            return {}

        return {symbol[0].ident: symbol[0] for symbol in symbols}

    def fq_ident(self, ident):
        return SEP.join([str(i) for i in ident])

    def type(self, type):
        return Type(type[0])

    def struct_def(self, struct_def):
        ident, fields = struct_def
        return StructDef(ident, fields)

    def struct_field(self, struct_field):
        ident, type = struct_field
        return StructField(ident, type)

    def func_def(self, func_def):
        ret_type = None
        param_list = []
        if len(func_def) == 3:
            if isinstance(func_def[-2], Type):
                ident, ret_type, body = func_def
            else:
                ident, param_list, body = func_def
        elif len(func_def) == 4:
            ident, param_list, ret_type, body = func_def
        else:
            ident, body = func_def

        return FuncDef(ident, param_list, ret_type, body)

    def func_param_list(self, params):
        return params

    def func_param(self, func_param):
        ident, type = func_param
        return FuncParam(ident, type)

    def postfix_expr(self, expr):
        return PostfixExpr(expr[0], expr[1:])

    def prefix_expr(self, expr):
        return PrefixExpr(expr[:-1], expr[-1])

    def prefix_plus(self, _):
        return Prefix.PLUS

    def prefix_neg(self, _):
        return Prefix.NEG

    def prefix_binv(self, _):
        return Prefix.BINV

    def prefix_not(self, _):
        return Prefix.NOT

    def prefix_ref(self, _):
        return Prefix.REF

    def prefix_deref(self, _):
        return Prefix.DEREF

    def func_call(self, func_call):
        args = []
        if len(func_call):
            args = func_call[0]
        return FuncCall(args)

    def get_attr(self, get_attr):
        return GetAttr(get_attr[0])

    def get_slice(self, get_slice):
        return GetSlice(get_slice[0])

    def range(self, range):
        range_start = None
        range_end = None
        range_incl = None

        for elem in range:
            if isinstance(elem, RangeStart):
                range_start = elem

            if isinstance(elem, RangeEnd):
                range_end = elem

            if isinstance(elem, RangeInclusive):
                range_incl = elem

        return Range(range_start, range_end, range_incl)

    def range_inclusive(self, range_incl):
        return RangeInclusive(True)

    def range_start(self, rstart):
        return RangeStart(rstart[0])

    def range_end(self, rend):
        return RangeEnd(rend[0])

    def arg_list(self, args):
        return args

    @v_args(meta=True)
    def symbol(self, meta, symbol_def):
        return symbol_def

    def block(self, block):
        return block

    def global_def(self, global_def):
        item_type, ident, type, expr = global_def
        return GlobalDef(ident, item_type, type, expr)

    def and_op(self, expr):
        return BinaryOp(BinOp.AND, *expr)

    def or_op(self, expr):
        return BinaryOp(BinOp.OR, *expr)

    def eq_op(self, expr):
        return BinaryOp(BinOp.EQ, *expr)

    def ne_op(self, expr):
        return BinaryOp(BinOp.NE, *expr)

    def gt_op(self, expr):
        return BinaryOp(BinOp.GT, *expr)

    def lt_op(self, expr):
        return BinaryOp(BinOp.LT, *expr)

    def gte_op(self, expr):
        return BinaryOp(BinOp.GTE, *expr)

    def lte_op(self, expr):
        return BinaryOp(BinOp.LTE, *expr)

    def bwor_op(self, expr):
        return BinaryOp(BinOp.BOR, *expr)

    def bwxor_op(self, expr):
        return BinaryOp(BinOp.BXOR, *expr)

    def bwand_op(self, expr):
        return BinaryOp(BinOp.BAND, *expr)

    def shl_op(self, expr):
        return BinaryOp(BinOp.SHL, *expr)

    def shr_op(self, expr):
        return BinaryOp(BinOp.SHR, *expr)

    def add_op(self, expr):
        return BinaryOp(BinOp.ADD, *expr)

    def sub_op(self, expr):
        return BinaryOp(BinOp.SUB, *expr)

    def mul_op(self, expr):
        return BinaryOp(BinOp.MUL, *expr)

    def div_op(self, expr):
        return BinaryOp(BinOp.DIV, *expr)

    def mod_op(self, expr):
        return BinaryOp(BinOp.MOD, *expr)

    def plus_equal(self, assign):
        return AsOp.ADDEQ

    def minus_equal(self, assign):
        return AsOp.SUBEQ

    def times_equal(self, assign):
        return AsOp.MULEQ

    def div_equal(self, assign):
        return AsOp.DIVEQ

    def mod_equal(self, assign):
        return AsOp.MODEQ

    def shl_equal(self, assign):
        return AsOp.SHLEQ

    def shr_equal(self, assign):
        return AsOp.SHREQ

    def bwand_equal(self, assign):
        return AsOp.ANDEQ

    def bwor_equal(self, assign):
        return AsOp.OREQ

    def equal(self, assign):
        return AsOp.EQ


class MainNotFound(Exception):
    pass


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
            if isinstance(symbol, FuncDef):
                self.main_exists = True
                return

        raise MainNotFound(
            f"elx missing entry point 'fn main' for project type {self.bin_type}"
        )

    def visit_symbol(self, ident: Identifier, item: Item):
        if isinstance(item, FuncDef):
            self.visit_func(item)

    def visit_module(self, module: Module):
        raise NotImplementedError()

    def visit_global(self, global_: GlobalDef):
        raise NotImplementedError()

    def visit_enum(self, enum_: EnumDef):
        raise NotImplementedError()

    def visit_block(self, block: list[Stmt]):
        raise NotImplementedError()

    def visit_func(self, func: FuncDef):
        raise NotImplementedError()

    def visit_assign_stmt(self, stmt: AssignStmt):
        raise NotImplementedError()

    def visit_let_stmt(self, stmt: LetStmt):
        raise NotImplementedError()

    def visit_return_stmt(self, stmt: ReturnStmt):
        raise NotImplementedError()

    def visit_expr_stmt(self, stmt: ExprStmt):
        raise NotImplementedError()

    def visit_for_stmt(self, stmt: ForStmt):
        raise NotImplementedError()

    def visit_while_stmt(self, stmt: WhileStmt):
        raise NotImplementedError()

    def visit_if_stmt(self, stmt: IfStmt):
        raise NotImplementedError()

    def visit_break_stmt(self, stmt: BreakStmt):
        raise NotImplementedError()

    def visit_continue_stmt(self, stmt: ContinueStmt):
        raise NotImplementedError()

    def visit_module_stmt(self, stmt: ModuleStmt):
        raise NotImplementedError()

    def analyze(self) -> dict:
        for ident, symbol in self.ast.items():
            self.visit_symbol(ident, symbol)

        return self.ast


class CLangTree:
    pass


class Transpiler:
    """
    Convert the IR to C
    """

    def __init__(self, ast: dict):
        self.ast = ast
        self.scope_depth = 0

    def transpile(self):
        pass


class ProjectType(Enum):
    BIN = 0
    LIB = 1


class OptLevel(Enum):
    DEBUG = 0
    RELEASE = 1
    RELWITHDEBUGINFO = 2


@dataclass
class CompilerOptions:
    input_path: Path
    level: OptLevel = OptLevel.DEBUG


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

    # func_def_w_body_ret_type = """
    # fn qux() -> null {
    #     module kez {
    #         let m = false;
    #         for j in iterator {
    #             m = true;
    #         }
    #     }
    #     let mut x = 0;
    #     let mut z: u32 = 4;
    #     x[..10].base.clear().reverse() * z;
    #     x && z;
    #     x += 1;
    #     let j = false;
    #     let y = 0 - -1 + (2 * 3) << (4 / 5);
    #     let y = x + 1;
    #     print(x);
    #     x.foo().bar.new();
    #     return;
    #     return x.new();
    # }
    # """
    #
    # pprint(parse(func_def_w_body_ret_type, parser, builder))
