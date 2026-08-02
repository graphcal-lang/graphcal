use crate::cancellation::{CancellationToken, Cancelled};
use crate::syntax::comments::SourceMetadata;
use crate::syntax::lexer::Lexer;
use crate::syntax::span::Span;
use crate::syntax::token::Token;

/// Maximum number of syntax tokens consumed between cancellation checkpoints.
pub(super) const CANCELLATION_CHECKPOINT_INTERVAL: usize = 256;

/// Parser-owned token stream that applies one cancellation policy to every
/// grammar production without threading a token through each parser helper.
#[derive(Clone)]
pub(super) struct ParserTokenStream<'src> {
    lexer: Lexer<'src>,
    cancellation: CancellationPolicy,
}

impl<'src> ParserTokenStream<'src> {
    pub(super) fn new(source: &'src str) -> Self {
        Self {
            lexer: Lexer::new(source),
            cancellation: CancellationPolicy::Unbounded,
        }
    }

    pub(super) fn enable_cancellation(&mut self, token: &CancellationToken) {
        self.cancellation = CancellationPolicy::bounded(CancellationProbe::Token(token.clone()));
    }

    pub(super) fn disable_cancellation(&mut self) {
        self.cancellation = CancellationPolicy::Unbounded;
    }

    /// Check immediately at an operation boundary, independently of the
    /// periodic token budget.
    pub(super) fn checkpoint(&mut self) -> Result<(), Cancelled> {
        self.cancellation.checkpoint()
    }

    pub(super) fn peek(&mut self) -> Option<&Token> {
        self.lexer.peek()
    }

    pub(super) fn peek_second(&mut self) -> Option<&Token> {
        self.lexer.peek_second()
    }

    pub(super) fn peek_third(&mut self) -> Option<&Token> {
        self.lexer.peek_third()
    }

    pub(super) fn peek_with_span(&mut self) -> Option<(&Token, Span)> {
        self.lexer.peek_with_span()
    }

    /// Consume a token after applying the parser's periodic cancellation
    /// checkpoint. Cancellation is represented as temporary end-of-input so
    /// existing parse helpers unwind through their normal error path; the
    /// cancellation-aware entry point then returns the latched typed outcome.
    pub(super) fn next_token(&mut self) -> Option<(Token, Span)> {
        if self.cancellation.before_next_token().is_err() {
            return None;
        }
        let token = self.lexer.next_token();
        if token.is_some() {
            self.cancellation.token_consumed();
        }
        token
    }

    pub(super) const fn first_error_span(&self) -> Option<Span> {
        self.lexer.first_error_span()
    }

    pub(super) fn slice_at(&self, span: Span) -> &'src str {
        self.lexer.slice_at(span)
    }

    pub(super) const fn source_len(&self) -> usize {
        self.lexer.source_len()
    }

    pub(super) fn into_source_metadata(self) -> SourceMetadata {
        self.lexer.into_source_metadata()
    }

    #[cfg(test)]
    pub(super) fn cancel_after_successful_checkpoints(&mut self, successful_checkpoints: usize) {
        self.cancellation = CancellationPolicy::bounded(
            CancellationProbe::CancelAfterSuccessfulCheckpoints(successful_checkpoints),
        );
    }
}

#[derive(Clone)]
enum CancellationPolicy {
    /// The ordinary parser path performs no atomic loads and has no token
    /// budget to maintain.
    Unbounded,
    Bounded {
        probe: CancellationProbe,
        tokens_before_checkpoint: usize,
        cancelled: bool,
    },
}

impl CancellationPolicy {
    const fn bounded(probe: CancellationProbe) -> Self {
        Self::Bounded {
            probe,
            tokens_before_checkpoint: 0,
            cancelled: false,
        }
    }

    fn checkpoint(&mut self) -> Result<(), Cancelled> {
        match self {
            Self::Unbounded => Ok(()),
            Self::Bounded {
                probe,
                tokens_before_checkpoint,
                cancelled,
            } => {
                if *cancelled || probe.checkpoint().is_err() {
                    *cancelled = true;
                    return Err(Cancelled);
                }
                *tokens_before_checkpoint = CANCELLATION_CHECKPOINT_INTERVAL;
                Ok(())
            }
        }
    }

    fn before_next_token(&mut self) -> Result<(), Cancelled> {
        match self {
            Self::Unbounded => Ok(()),
            Self::Bounded {
                tokens_before_checkpoint,
                ..
            } if *tokens_before_checkpoint > 0 => Ok(()),
            Self::Bounded { .. } => self.checkpoint(),
        }
    }

    fn token_consumed(&mut self) {
        match self {
            Self::Unbounded => {}
            Self::Bounded {
                tokens_before_checkpoint,
                ..
            } => {
                debug_assert!(*tokens_before_checkpoint > 0);
                *tokens_before_checkpoint = tokens_before_checkpoint.saturating_sub(1);
            }
        }
    }
}

#[derive(Clone)]
enum CancellationProbe {
    Token(CancellationToken),
    #[cfg(test)]
    CancelAfterSuccessfulCheckpoints(usize),
}

impl CancellationProbe {
    fn checkpoint(&mut self) -> Result<(), Cancelled> {
        match self {
            Self::Token(token) => token.checkpoint(),
            #[cfg(test)]
            Self::CancelAfterSuccessfulCheckpoints(remaining) => {
                if *remaining == 0 {
                    Err(Cancelled)
                } else {
                    *remaining -= 1;
                    Ok(())
                }
            }
        }
    }
}
