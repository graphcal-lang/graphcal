use crate::cancellation::{CancellationToken, Cancelled};
use crate::syntax::comments::SourceMetadata;
use crate::syntax::lexer::{TokenizedSource, tokenize};
use crate::syntax::span::Span;
use crate::syntax::token::Token;

/// Maximum number of syntax tokens consumed between cancellation checkpoints.
pub(super) const CANCELLATION_CHECKPOINT_INTERVAL: usize = 256;

/// Parser-owned token stream that applies one cancellation policy to every
/// grammar production without threading a token through each parser helper.
///
/// The source is tokenized once up front, so the stream state that moves
/// during parsing is only a [`Cursor`] into an immutable token slice. Comments
/// and blank lines live in the immutable [`TokenizedSource`]: lookahead and
/// speculative parsing (see [`Self::mark`] and [`Self::rewind`]) therefore
/// cost O(1) regardless of how much trivia the source records, and a rewound
/// speculation cannot leave recorded trivia behind.
///
/// The stream is intentionally not `Clone`; use [`Self::upcoming`] for
/// read-only lookahead and a [`TokenMark`] for speculation.
pub(super) struct ParserTokenStream<'src> {
    source: &'src str,
    tokenized: TokenizedSource,
    cursor: Cursor,
    cancellation: CancellationPolicy,
}

/// The only mutable parsing position of a [`ParserTokenStream`].
#[derive(Clone, Copy)]
struct Cursor {
    /// Index of the next unconsumed token.
    next: usize,
    /// The span of the last token consumed, where a production that just
    /// finished ends.
    last_consumed: Option<Span>,
}

/// A saved stream position, produced by [`ParserTokenStream::mark`] and
/// consumed by [`ParserTokenStream::rewind`].
///
/// It is neither `Clone` nor `Copy`, so each mark rewinds at most once, and
/// its fields are private, so no position can be fabricated.
pub(super) struct TokenMark(Cursor);

impl<'src> ParserTokenStream<'src> {
    pub(super) fn new(source: &'src str) -> Self {
        Self {
            source,
            tokenized: tokenize(source),
            cursor: Cursor {
                next: 0,
                last_consumed: None,
            },
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

    fn remaining(&self) -> &[(Token, Span)] {
        // `next` only advances by consuming an existing token and is only
        // rewound to an earlier mark, so it never exceeds the token count.
        &self.tokenized.tokens()[self.cursor.next..]
    }

    fn peek_with_span_at(&self, offset: usize) -> Option<(&Token, Span)> {
        self.remaining()
            .get(offset)
            .map(|(token, span)| (token, *span))
    }

    pub(super) fn peek(&self) -> Option<&Token> {
        self.peek_with_span_at(0).map(|(token, _)| token)
    }

    pub(super) fn peek_second(&self) -> Option<&Token> {
        self.peek_with_span_at(1).map(|(token, _)| token)
    }

    pub(super) fn peek_third(&self) -> Option<&Token> {
        self.peek_with_span_at(2).map(|(token, _)| token)
    }

    pub(super) fn peek_with_span(&self) -> Option<(&Token, Span)> {
        self.peek_with_span_at(0)
    }

    /// Read-only lookahead over every unconsumed token, in source order.
    pub(super) fn upcoming(&self) -> impl Iterator<Item = Token> + Clone + '_ {
        self.remaining().iter().map(|(token, _)| *token)
    }

    /// Save the current position so a speculative parse can be undone.
    pub(super) const fn mark(&self) -> TokenMark {
        TokenMark(self.cursor)
    }

    /// Return to a position saved by [`Self::mark`]. The cancellation budget
    /// is deliberately not restored: tokens consumed speculatively still
    /// count towards the next checkpoint, and a latched cancellation stays
    /// latched.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "consuming the mark makes every mark single-use"
    )]
    pub(super) const fn rewind(&mut self, mark: TokenMark) {
        self.cursor = mark.0;
    }

    /// Consume a token after applying the parser's periodic cancellation
    /// checkpoint. Cancellation is represented as temporary end-of-input so
    /// existing parse helpers unwind through their normal error path; the
    /// cancellation-aware entry point then returns the latched typed outcome.
    pub(super) fn next_token(&mut self) -> Option<(Token, Span)> {
        if self.cancellation.before_next_token().is_err() {
            return None;
        }
        let token = self.remaining().first().copied();
        if let Some((_, span)) = token {
            self.cancellation.token_consumed();
            self.cursor = Cursor {
                next: self.cursor.next + 1,
                last_consumed: Some(span),
            };
        }
        token
    }

    /// The span of the last token consumed.
    pub(super) const fn last_consumed(&self) -> Option<Span> {
        self.cursor.last_consumed
    }

    pub(super) const fn first_error_span(&self) -> Option<Span> {
        self.tokenized.first_error_span()
    }

    pub(super) fn slice_at(&self, span: Span) -> &'src str {
        &self.source[span.offset()..span.offset() + span.len()]
    }

    pub(super) const fn source_len(&self) -> usize {
        self.source.len()
    }

    pub(super) fn into_source_metadata(self) -> SourceMetadata {
        self.tokenized.into_source_metadata()
    }

    pub(super) const fn source_metadata(&self) -> &SourceMetadata {
        self.tokenized.source_metadata()
    }

    #[cfg(test)]
    pub(super) fn cancel_after_successful_checkpoints(&mut self, successful_checkpoints: usize) {
        self.cancellation = CancellationPolicy::bounded(
            CancellationProbe::CancelAfterSuccessfulCheckpoints(successful_checkpoints),
        );
    }
}

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
