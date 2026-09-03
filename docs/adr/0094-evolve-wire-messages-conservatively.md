# Evolve wire messages conservatively

Compatible protocol minors may add optional object fields that older readers ignore. Unknown message kinds, Commands, and security-sensitive variants are rejected rather than guessed, while unknown Presentation blocks are retained opaquely and rendered generically. Contract fixtures exercise current and previous supported majors across Rust, Swift, and TypeScript so permissive JSON parsing does not weaken command validation.
