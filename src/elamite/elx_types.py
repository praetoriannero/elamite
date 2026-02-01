from __future__ import annotations
from dataclasses import dataclass
from lark.tree import Meta
from enum import Enum


SEP = "__"


class BinOp(Enum):
    OR = "||"
    AND = "&&"
    EQ = "=="
    NE = "!="
    GT = ">"
    LT = "<"
    GTE = ">="
    LTE = "<="
    BOR = "|"
    BXOR = "^"
    BAND = "&"
    SHL = "<<"
    SHR = ">>"
    ADD = "+"
    SUB = "-"
    MUL = "*"
    DIV = "/"
    MOD = "%"


class AsOp(Enum):
    ADDEQ = "+="
    SUBEQ = "-="
    MULEQ = "*="
    DIVEQ = "/="
    MODEQ = "%="
    SHLEQ = "<<="
    SHREQ = ">>="
    ANDEQ = "&="
    OREQ = "|="
    XOREQ = "^="
    EQ = "="


class UnOp(Enum):
    POS = "+"
    NEG = "-"
    BNOT = "~"
    NOT = "!"
    REF = "&"
    DER = "*"


type Expr = Identifier | Integer | Float | Bool | String | FuncCall | BinaryOp | UnaryOp


type PostFix = GetAttr | GetSlice | FuncCall


type Item = FuncDef | StructDef | GlobalDef | EnumDef | Module | Import

type Node = (
    StructDef
    | FuncDef
    | GlobalDef
    | EnumDef
    | Module
    | Import
    | AssignStmt
    | LetStmt
    | ReturnStmt
    | ExprStmt
    | ForStmt
    | WhileStmt
    | IfStmt
    | BreakStmt
    | ContinueStmt
)


@dataclass(unsafe_hash=True)
class Symbol:
    """
    Parent class for defining symbols, top level items in a module.
    """

    meta: Meta
    ident: Identifier


@dataclass(unsafe_hash=True)
class Module(Symbol):
    types: dict[str, StructDef | EnumDef]
    funcs: dict[str, FuncDef]
    globals: dict[str, GlobalDef]
    mods: dict[str, Module]
    imports: dict[str, Import]


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
    block: list[Stmt]


class GlobalKind(Enum):
    CONST = "const"
    STATIC = "static"


@dataclass
class GlobalDef(Symbol):
    kind: GlobalKind
    mut: bool
    type: Type
    expr: Expr


@dataclass
class Import(Symbol):
    ident: Identifier


@dataclass
class FuncParam:
    ident: Identifier
    type: Type


@dataclass
class StructField:
    ident: str
    type: str


@dataclass
class Stmt:
    meta: Meta


@dataclass
class AssignStmt(Stmt):
    op: AssignOp
    lhs: Identifier
    rhs: Expr


@dataclass
class LetStmt(Stmt):
    ident: Identifier
    type: Type | None
    expr: Expr
    mut: bool


@dataclass
class ReturnStmt(Stmt):
    expr: Expr | None


@dataclass
class ExprStmt(Stmt):
    expr: Expr


@dataclass
class ForStmt(Stmt):
    ident: Identifier
    iterator: Expr
    block: list[Stmt]


@dataclass
class WhileStmt(Stmt):
    expr: Expr
    block: list[Stmt]


@dataclass
class IfStmt(Stmt):
    expr: Expr
    block: list[Stmt]
    elif_clauses: list[ElifClause]
    else_clause: ElseClause | None


@dataclass
class ElifClause(Stmt):
    expr: Expr
    block: list[Stmt]


@dataclass
class ElseClause(Stmt):
    block: list[Stmt]


@dataclass
class BreakStmt(Stmt):
    expr: Expr | None


@dataclass
class ContinueStmt(Stmt):
    expr: Expr | None


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


@dataclass
class Unit(Type):
    ident: Identifier = Identifier("unit")


@dataclass
class Array:
    size: Expr
    type: Type


@dataclass
class String:
    value: str


@dataclass
class PostfixExpr:
    ident: Identifier
    postfixes: list[PostFix]


@dataclass
class PrefixExpr:
    prefixes: list[Prefix]
    ident: Identifier


class Prefix(Enum):
    PLUS = "+"
    NEG = "-"
    BINV = "~"
    NOT = "!"
    REF = "&"
    DEREF = "*"


@dataclass
class FuncCall:
    args: list[Expr]


@dataclass
class GetAttr:
    ident: Identifier


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
class Range:
    start: RangeStart | None
    end: RangeEnd | None
    inclusive: RangeInclusive | None


@dataclass
class GetSlice:
    slice: Range | Expr
