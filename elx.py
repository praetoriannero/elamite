from __future__ import annotations

from enum import Enum
from dataclasses import dataclass
from lark import Lark, Transformer, v_args
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
    params: list[FuncParam]
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
class LetStmt:
    ident: Identifier
    type: Type | None
    value: str
    mut: bool


@dataclass
class AssignStmt:
    op: AssignOp
    lhs: Identifier
    rhs: Expr


@dataclass
class ReturnStmt:
    expr: Expr


@dataclass
class ExprStmt:
    expr: Expr


@dataclass
class ForStmt:
    pass


@dataclass(frozen=True)
class Identifier:
    name: str


@dataclass
class Integer:
    value: str


@dataclass
class Float:
    value: str


@dataclass
class Bool:
    value: bool


type Expr = Identifier | Integer | Float | Bool | String | FuncCall | BinaryOp | UnaryOp


type PostFix = GetAttr | GetSlice | FuncCall


@dataclass
class String:
    value: str


@dataclass
class AtomExpr:
    ident: Identifier
    postfixes: list[PostFix]


@dataclass
class FuncCall:
    args: list[Expr]


@dataclass
class GetAttr:
    ident: Identifier


@dataclass
class GetSlice:
    slice: Range | Expr


@dataclass
class Range:
    start: RangeStart | None
    end: RangeEnd | None
    inclusive: RangeInclusive | None


@dataclass
class RangeStart:
    value: Expr


@dataclass
class RangeInclusive:
    is_incl: bool


@dataclass
class RangeEnd:
    value: Expr


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

    def stmt(self, stmt):
        return stmt[0]

    def let_stmt(self, stmt):
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

        return LetStmt(ident, type, expr, mut)

    def expr_stmt(self, stmt):
        return ExprStmt(*stmt)

    def mut(self, mut):
        return "mut"

    def bool_true(self, atom):
        return Bool(True)

    def bool_false(self, atom):
        return Bool(False)

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

    def atom_expr(self, expr):
        return AtomExpr(expr[0], expr[1:])

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

    def global_stmt(self, global_stmt):
        item_type, ident, type, expr = global_stmt
        return GlobalStmt(ident, item_type, type, expr)

    def assign_stmt(self, assign_stmt):
        lhs, op, rhs = assign_stmt
        return AssignStmt(op, lhs, rhs)

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
        self.scope = {}  # our current namespace
        self.locals: dict[Identifier, Type] = {}  # exist in the current scope
        self.globals: dict[Identifier, Type] = {}  # exist in the global scope

    def visit_symbol(self, symbol: Symbol):
        pass

    def visit_func(self, func: FuncDef):
        pass

    def analyze(self) -> dict:
        return {}


class Transpiler:
    """
    Convert the IR to C
    """

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
    level: OptLevel


class Compiler:
    """
    Compile the code
    """

    def __init__(self, ast: dict, options: CompilerOptions):
        pass


if __name__ == "__main__":
    lines = None
    with open("elamite.lark", "r") as input_handle:
        lines = input_handle.read()

    parser = Lark(lines, propagate_positions=True, parser="lalr")
    builder = Builder()

    def parse(source, parser, builder):
        tree = parser.parse(source)
        return builder.transform(tree)

    empty_example = ""
    pprint(parse(empty_example, parser, builder))

    struct_def_example = "struct foo{a: qux,}"
    ast = parse(struct_def_example, parser, builder)
    pprint(ast)

    func_def_example = "fn qux(a: bar, b: baz) {}"
    ast = parse(func_def_example, parser, builder)
    pprint(ast)

    func_def_w_return_type = "fn qux() -> baz {}"
    pprint(parse(func_def_w_return_type, parser, builder))

    multi_symbol_example = struct_def_example + " " + func_def_example
    pprint(parse(multi_symbol_example, parser, builder))

    assignment_example = "const y: f32 = 10.0;"
    pprint(parse(assignment_example, parser, builder))

    func_def_w_body_ret_type = """
    fn qux() -> null {
        let mut x = 0;
        let mut z: u32 = 4;
        x[..10].base.clear().reverse();
        x && z;
        x += 1;
        let j = false;
        let y = 0 - -1 + (2 * 3) << (4 / 5);
        let y = x + 1;
        print(x);
        x.foo().bar.new();
    }
    """
    pprint(parse(func_def_w_body_ret_type, parser, builder))
