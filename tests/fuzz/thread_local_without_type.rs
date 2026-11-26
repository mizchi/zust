// thread_local! with no type before a closing delimiter (lexer.rs expand_thread_local, an empty token range).
thread_local!{static STACK: ]
