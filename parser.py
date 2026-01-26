from ply.lex import lex
from ply.yacc import yacc
from pprint import pprint


reserved_kw = {
    "import": "IMPORT",
    "struct": "STRUCT",
    "enum": "ENUM",
    "impl": "IMPL",
    "default": "DEFAULT",
    "fn": "FN",
    "return": "RETURN",
    "union": "UNION",
}

reserved_types = {
    "i8": "I8",
    "i16": "I16",
    "i32": "I32",
    "i64": "I64",
    "i128": "I128",
    "u8": "U8",
    "u16": "U16",
    "u32": "U32",
    "u64": "U64",
    "u128": "U128",
    "f32": "F32",
    "f64": "F64",
    "f128": "F128",
    "string": "STRING",
}

tokens = (
    [
        "IDENT",
        "LBRACE",
        "RBRACE",
        "LPAREN",
        "RPAREN",
        "LANGLE",
        "RANGLE",
        "RARROW",
        "COLON",
        "COMMA",
        "ASSIGN",
        "SEMI",
        "UINT",
    ]
    + list(reserved_kw.values())
    + list(reserved_types.values())
)

t_LBRACE = r"\{"
t_RBRACE = r"\}"
t_LPAREN = r"\("
t_RPAREN = r"\)"
t_LANGLE = r"<"
t_RANGLE = r">"
t_RARROW = r"->"
t_COLON = r":"
t_COMMA = r","
t_ASSIGN = r"="
t_SEMI = r";"


def t_IDENT(t):
    r"[a-zA-Z_][a-zA-Z_0-9]*"
    t.type = reserved_kw.get(t.value, "IDENT")  # Check for reserved words
    return t


def t_UINT(t):
    r"\d+"
    t.value = ("uint", int(t.value))
    return t


def t_newline(t):
    r"\n+"
    t.lexer.lineno += len(t.value)


t_ignore = " \t\n"


def t_error(t):
    raise ValueError(f"Illegal character {t.value[0]} on line {t.lexer.lineno}")


# def p_empty(p):
#     """
#     empty :
#     """
#     pass


def p_symbol_list(p):
    """
    symbol_list : symbol_list symbol
                | symbol
    """
    p[0] = [p[1]]


def p_symbol(p):
    """
    symbol : struct_decl
           | enum_decl
           | fn_decl
           | default_decl
           | impl_decl
    """
    p[0] = p[1]


def p_struct_decl(p):
    """
    struct_decl : STRUCT IDENT LBRACE field_list RBRACE
    """
    p[0] = {"kind": "STRUCT", "ident": p[2], "field_list": p[4]}


def p_enum_decl(p):
    """
    enum_decl : ENUM IDENT LBRACE variant_list RBRACE
    """
    p[0] = {"kind": "ENUM", "ident": p[2], "variant_list": p[4]}


def p_variant_list(p):
    """
    variant_list : variant_list variant
                 | variant
    """
    raise NotImplementedError()


def p_variant(p):
    """
    variant : IDENT
    """
    raise NotImplementedError()


def p_fn_decl(p):
    """
    fn_decl : FN IDENT LPAREN RPAREN RARROW IDENT LBRACE stmnt_list RBRACE
           | FN IDENT LPAREN RPAREN LBRACE stmnt_list RBRACE
    """
    # fn foo() -> bar {}
    raise NotImplementedError()


def p_declault_decl(p):
    """
    default_decl : DEFAULT IDENT LBRACE RBRACE
    """
    # default Example {}
    raise NotImplementedError()


def p_impl_decl(p):
    """
    impl_decl : IMPL IDENT LBRACE RBRACE
    """
    raise NotImplementedError()


def p_stmnt_list(p):
    """
    stmnt_list : stmnt_list stmnt
               | stmnt
    """
    raise NotImplementedError()


def p_stmnt(p):
    """
    stmnt : abs_expr
          | jump_expr
    """
    # could also be for, while, if, etc.
    raise NotImplementedError()


def p_abs_expr(p):
    """
    abs_expr : literal SEMI
             | IDENT ASSIGN IDENT SEMI
             | IDENT ASSIGN literal SEMI
    """
    p[0] = p[1]


def p_jump_expr(p):
    """
    jump_expr : RETURN abs_expr SEMI
    """
    p[0] = p[1]


def p_literal(p):
    """
    literal : UINT
    """
    p[0] = p[1]


def p_field_list(p):
    """
    field_list : field_list field
               | field
    """
    if len(p) == 3:
        p[0] = p[1] + [p[2]]
    else:
        p[0] = [p[1]]


def p_field(p):
    """
    field : IDENT COLON IDENT COMMA
    """
    p[0] = {
        "ident": p[1],
        "type": p[3],
    }


def p_error(p):
    print("ERROR PARSING:", p)


class SymbolTable:
    def __init__(self, symbol_decl: dict):
        pass


class Struct:
    def __init__(self, struct_decl: dict):
        self.ident = struct_decl["ident"]
        self.fields = struct_decl["field_list"]


class Enum:
    def __init__(self, enum_decl: dict):
        pass


class Func:
    def __init__(self, func_decl: dict):
        pass


class Ref:
    def __init__(self, ref_decl: dict):
        pass


class Ptr:
    def __init__(self, ptr_decl: dict):
        pass


class Expr:
    pass


class Return:
    def __init__(self, expr: Expr):
        self.expr = expr


if __name__ == "__main__":
    struct_data = """
    struct Example {
        a: i32,
        b: f16,
        c: string,
        d: u64,
    }

    fn main() {
        return 0;
    }
    """
    print(struct_data)

    lexer = lex()
    parser = yacc()
    ast = parser.parse(struct_data)
    pprint(ast, sort_dicts=False)
