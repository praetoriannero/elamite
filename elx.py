from __future__ import annotations

from enum import Enum
from dataclasses import dataclass
from lark import Lark, Transformer
from pathlib import Path
from pprint import pprint

SEP = "__"


@dataclass(unsafe_hash=True)
class Symbol:
    ident: str


@dataclass
class Module(Symbol):
    symbols: dict


@dataclass
class StructDef(Symbol):
    fields: list


@dataclass
class EnumDef(Symbol):
    variants: list


@dataclass
class FuncDef(Symbol):
    params: list[FuncParam] | None
    ret_type: Type | None
    body: list


@dataclass
class GlobalStmt(Symbol):
    item_type: str
    type: str
    expr: str


@dataclass
class FuncParam:
    ident: str
    type: str


@dataclass
class StructField:
    ident: str
    type: str


@dataclass
class VarDecl:
    ident: Identifier
    type: Type
    value: str
    mut: bool


@dataclass
class VarAssign:
    op: AssignOp
    lhs: Identifier
    rhs: Expr


@dataclass(frozen=True)
class Identifier:
    name: str


@dataclass
class Integer:
    value: str


@dataclass
class Float:
    value: str


type Expr = Identifier | Integer | Float | String | FuncCall | BinaryOp | UnaryOp


@dataclass
class String:
    value: str


@dataclass
class FuncCall:
    ident: Identifier
    args: list[Expr]


@dataclass
class BinaryOp:
    op: BinOp
    lhs: Expr
    rhs: Expr


@dataclass
class UnaryOp:
    op: UnOp
    inner: Expr


@dataclass
class AssignOp:
    op: AsOp
    lhs: Expr
    rhs: Expr


@dataclass
class Type:
    ident: Identifier


class BinOp(Enum):
    Or = "||"
    And = "&&"
    Eq = "=="
    Ne = "!="
    Gt = ">"
    Lt = "<"
    Gte = ">="
    Lte = "<="
    Bor = "|"
    Bxor = "^"
    Band = "&"
    Shl = "<<"
    Shr = ">>"
    Add = "+"
    Sub = "-"
    Mul = "*"
    Div = "/"
    Mod = "%"


class AsOp(Enum):
    AddEq = "+="
    SubEq = "-="
    MulEq = "*="
    DivEq = "/="
    ModEq = "%="
    ShlEq = "<<="
    ShrEq = ">>="
    AndEq = "&="
    OrEq = "|="
    XorEq = "^="
    Eq = "="


class UnOp(Enum):
    Pos = "+"
    Neg = "-"
    Bnot = "~"
    Not = "!"
    Ref = "&"
    Der = "*"


class Tree(Transformer):
    NULL = str
    MUT = str

    def ident(self, ident):
        return Identifier(str(ident[0]))

    def func_call_expr(self, expr):
        args = [] if len(expr) == 1 else expr[1]
        return FuncCall(expr[0], args)

    def integer(self, integer):
        value = str(integer[0])
        return Integer(value)

    def float(self, float):
        value = str(float[0])
        return Float(value)

    def stmt(self, stmt):
        return stmt[0]

    def init_stmt(self, stmt):
        def _recover_type(expr):
            # TODO
            return None

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
            type = _recover_type(stmt[idx])

        return VarDecl(ident, type, expr, mut)

    def mut(self, mut):
        return "mut"

    def start(self, module):
        return module[0]

    def module(self, symbols):
        if not symbols:
            return {}

        return {Symbol(symbol[0].ident): symbol[0] for symbol in symbols}

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
        param_list = None
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

    def func_arg_list(self, args):
        return args

    def symbol(self, symbol_def):
        return symbol_def

    def block(self, block):
        return block

    def global_stmt(self, global_stmt):
        item_type, ident, type, expr = global_stmt
        return GlobalStmt(ident, item_type, type, expr)

    def assign_stmt(self, assign_stmt):
        lhs, op, rhs = assign_stmt
        return VarAssign(op, lhs, rhs)

    def and_op(self, expr):
        return BinaryOp(BinOp.And, *expr)

    def or_op(self, expr):
        return BinaryOp(BinOp.Or, *expr)

    def eq_op(self, expr):
        return BinaryOp(BinOp.Eq, *expr)

    def ne_op(self, expr):
        return BinaryOp(BinOp.Ne, *expr)

    def gt_op(self, expr):
        return BinaryOp(BinOp.Gt, *expr)

    def lt_op(self, expr):
        return BinaryOp(BinOp.Lt, *expr)

    def gte_op(self, expr):
        return BinaryOp(BinOp.Gte, *expr)

    def lte_op(self, expr):
        return BinaryOp(BinOp.Lte, *expr)

    def bwor_op(self, expr):
        return BinaryOp(BinOp.Bor, *expr)

    def bwxor_op(self, expr):
        return BinaryOp(BinOp.Bxor, *expr)

    def bwand_op(self, expr):
        return BinaryOp(BinOp.Band, *expr)

    def shl_op(self, expr):
        return BinaryOp(BinOp.Shl, *expr)

    def shr_op(self, expr):
        return BinaryOp(BinOp.Shr, *expr)

    def add_op(self, expr):
        return BinaryOp(BinOp.Add, *expr)

    def sub_op(self, expr):
        return BinaryOp(BinOp.Sub, *expr)

    def mul_op(self, expr):
        return BinaryOp(BinOp.Mul, *expr)

    def div_op(self, expr):
        return BinaryOp(BinOp.Div, *expr)

    def mod_op(self, expr):
        return BinaryOp(BinOp.Mod, *expr)

    def factor_plus(self, factor):
        return UnaryOp(UnOp.Pos, factor[0])

    def factor_neg(self, factor):
        return UnaryOp(UnOp.Neg, factor[0])

    def factor_binv(self, factor):
        return UnaryOp(UnOp.Bnot, factor[0])

    def factor_not(self, factor):
        return UnaryOp(UnOp.Not, factor[0])

    def factor_ref(self, factor):
        return UnaryOp(UnOp.Ref, factor[0])

    def factor_deref(self, factor):
        return UnaryOp(UnOp.Der, factor[0])

    def plus_equal(self, assign):
        return AsOp.AddEq

    def minus_equal(self, assign):
        return AsOp.SubEq

    def times_equal(self, assign):
        return AsOp.MulEq

    def div_equal(self, assign):
        return AsOp.DivEq

    def mod_equal(self, assign):
        return AsOp.ModEq

    def shl_equal(self, assign):
        return AsOp.ShlEq

    def shr_equal(self, assign):
        return AsOp.ShrEq

    def bwand_equal(self, assign):
        return AsOp.AndEq

    def bwor_equal(self, assign):
        return AsOp.OrEq

    def equal(self, assign):
        return AsOp.Eq


class Analyzer:
    def __init__(self, ast: dict):
        pass

    def visit_symbol(self, symbol: Symbol):
        pass

    def visit_func(self, func: FuncDef):
        pass


class OptLevel(Enum):
    DEBUG = 0
    RELEASE = 1
    RELWITHDEBUGINFO = 2


@dataclass
class CompilerOptions:
    input_path: Path
    level: OptLevel


class Compiler:
    def __init__(self, ast: Tree, options: CompilerOptions):
        pass


if __name__ == "__main__":
    lines = None
    with open("elamite.lark", "r") as input_handle:
        lines = input_handle.read()

    builder = Lark(lines, transformer=Tree(), parser="lalr")

    empty_example = ""
    pprint(builder.parse(empty_example))

    struct_def_example = "struct foo{a: qux,}"
    ast = builder.parse(struct_def_example)
    pprint(ast)

    func_def_example = "fn qux(a: bar, b: baz) {}"
    ast = builder.parse(func_def_example)
    pprint(ast)

    func_def_w_return_type = "fn qux() -> baz {}"
    pprint(builder.parse(func_def_w_return_type))

    multi_symbol_example = struct_def_example + " " + func_def_example
    pprint(builder.parse(multi_symbol_example))

    assignment_example = "const y: f32 = 10.0;"
    pprint(builder.parse(assignment_example))

    func_def_w_body_ret_type = """
    fn qux() -> null {
        let mut x = 0;
        let mut z: u32 = 4;
        x && z;
        x += 1;
        let y = 0 - -1 + (2 * 3) << (4 / 5);
        let y = x + 1;
        print(x);
    }
    """
    pprint(builder.parse(func_def_w_body_ret_type))
