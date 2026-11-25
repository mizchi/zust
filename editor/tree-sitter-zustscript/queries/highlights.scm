; zustscript highlights (capture names as in nvim-treesitter / Helix)

(line_comment) @comment
(block_comment) @comment

(string) @string
(template_string "`" @string)
(string_fragment) @string
(escape_sequence) @string.escape
(template_substitution ["${" "}"] @punctuation.special)
(char) @string
(jsx_attribute value: (string) @string)
(number) @number
(boolean) @constant.builtin
(wildcard) @variable.builtin

[
  "type" "enum" "fn" "extern" "import" "from" "wasm" "let" "mut" "if" "else" "match" "while" "loop" "for" "in"
  "as" "perform" "handle" "return" "async" "await"
] @keyword
(visibility) @keyword
(mutable) @keyword
(break_expression) @keyword
(continue_expression) @keyword
(rust_keyword) @keyword
(attribute "@" @attribute)
(attribute (attribute_name) @attribute)

(primitive_type) @type.builtin
(type_identifier) @type
(import_name) @variable
(variant_identifier) @constructor
(constructor) @constructor

(function_item name: (function_identifier) @function)
(extern_item name: (function_identifier) @function)
(extern_type_item name: (type_identifier) @type)
(extern_method name: (function_identifier) @function.method)
(extern_property name: (field_identifier) @property)
"?" @operator
(wasm_function_item name: (function_identifier) @function)
(call_expression function: (identifier) @function.call)
(call_expression function: (field_expression field: (field_identifier) @function.method))
(pipe_expression method: (method_identifier) @function.method)
(path_expression name: (identifier) @function.call)
(rust_macro name: (identifier) @function.macro "!" @function.macro)

(parameter name: (identifier) @variable.parameter)
(closure_parameter (identifier) @variable.parameter)
(field_identifier) @property
(shorthand_field) @property

(jsx_opening_element name: (jsx_tag) @tag)
(jsx_closing_element name: (jsx_tag) @tag)
(jsx_self_closing_element name: (jsx_tag) @tag)
(jsx_attribute_name) @attribute
(jsx_text) @text

[
  "=" "+=" "-=" "*=" "/=" "%=" "==" "!=" "<=" ">=" "&&" "||" "+" "-" "*" "/" "%" "!" "|>" "=>" "->" ".." "::" "&"
] @operator
(rust_punct) @operator
(binary_expression operator: ["<" ">"] @operator)

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," ";" ":" "."] @punctuation.delimiter
(jsx_opening_element ["<" ">"] @tag.delimiter)
(jsx_closing_element ["<" "/" ">"] @tag.delimiter)
(jsx_self_closing_element ["<" "/" ">"] @tag.delimiter)
(jsx_fragment ["<" "/" ">"] @tag.delimiter)

(identifier) @variable
