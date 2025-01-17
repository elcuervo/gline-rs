use crate::util::result::Result;
use crate::{text::{token::Token, tokenizer::Tokenizer}, util::compose::Composable};
use super::prompt::PromptInput;
use ndarray::{Array, Array2, ArrayView};

/// Represents encoded prompts (after sub-word tokenization)
pub struct EncodedInput {
    pub texts: Vec<String>,
    pub tokens: Vec<Vec<Token>>,
    pub entities: Vec<String>,
    pub num_words: usize,
    pub num_tokens: usize,
    pub input_ids: Array2<i64>,
    pub attention_masks: Array2<i64>,
    pub word_masks: Array2<i64>,
    pub text_lengths: Array2<i64>,
}

/// Utility struct
struct EncodedPrompt {
    /// encodings of each word
    encoding: Vec<Vec<u32>>,
    /// offset of the first token of the actual text (beside entity labels)
    text_offset: usize,
}

impl EncodedInput {

    // Each word of each prompt is encoded *one by one*. So each word generates an encoding as 
    // a Vec<u32> (sub-word tokenization). So for each prompt we get a Vec<Vec<u32>> (which is 
    // stored in the 'encoding' field).
    pub fn from(input: PromptInput, tokenizer: &impl Tokenizer) -> Result<Self> {        
        // prepare the result vector
        let mut encodings: Vec<EncodedPrompt> = Vec::with_capacity(input.prompts.len());
        // maximum number of sub-word tokens found in one prompt (will be the width of the input tensor)
        let mut max_tokens: usize = 0;
        // process each prompt
        for prompt in &input.prompts {
            // resulting sequence of encodings for each word of the current prompt
            let mut prompt_tokens: Vec<Vec<u32>> = Vec::with_capacity(prompt.tokens().len());
            // total number of sub-word tokens for the current prompt (adding 2 for initial and terminal tokens)
            let mut total_tokens: usize = 2;
            // number of sub-word tokens for the entities part only (before the actual text)
            let mut total_entity_tokens = 0;
            // encode each token of the current prompt
            for (pos, word) in prompt.tokens().iter().enumerate() {
                // actually encode the word
                let encoding = tokenizer.encode(word)?;
                // increment the number of sub-word tokens accordingly
                total_tokens += encoding.len();
                // increment the number of sub-word tokens in the entity part (will be used to start the word masks at the right place)
                if pos <= prompt.entities_len() {
                    total_entity_tokens += encoding.len();
                }
                prompt_tokens.push(encoding);
            }
            // update global result: push encoded prompt and update max_tokens
            encodings.push(EncodedPrompt { encoding: prompt_tokens, text_offset: total_entity_tokens });
            max_tokens = std::cmp::max(max_tokens, total_tokens);
        }

        // Compute vectors for each prompt. The `encoding` structure (which is 
        // word by word) gets flattened, but the word-level information is 
        // still represented by the "word mask".
        let mut input_ids = Array::zeros((0, max_tokens));
        let mut attention_masks = Array::zeros((0, max_tokens));
        let mut word_masks = Array::zeros((0, max_tokens));
        for encoded_prompt in encodings {
            let encoding = encoded_prompt.encoding;
            let mut input_id = vec!(0i64; max_tokens);
            let mut attn_mask = vec!(0i64; max_tokens);
            let mut word_mask = vec!(0i64; max_tokens);            

            let mut idx: usize = 0;
            let mut word_id: i64 = 0;

            // add initial token
            input_id[idx] = 1;
            attn_mask[idx] = 1;            
            idx += 1;

            // process each encoded (sub-word) token
            for word in encoding {
                for token in word {
                    input_id[idx] = token as i64;
                    // attention mask
                    attn_mask[idx] = 1;
                    // word mask (only for non-label tokens)
                    if idx >= encoded_prompt.text_offset {
                        word_mask[idx] = word_id;
                    }
                    // update position
                    idx += 1;
                }
                // increment word mask (if we are over the label tokens)
                if idx >= encoded_prompt.text_offset {
                    word_id += 1;
                }
            }

            // add terminal token
            input_id[idx] = 2;
            attn_mask[idx] = 1;

            // update final results
            input_ids.push_row(ArrayView::from(&input_id))?;
            attention_masks.push_row(ArrayView::from(&attn_mask))?;
            word_masks.push_row(ArrayView::from(&word_mask))?;
        }

        // text lengths (this data is fundamentally one-dimensional, but the model expects a two-dimensional one)
        let mut text_lengths = Array::zeros((0, 1));
        for text_length in input.text_lengths {
            text_lengths.push_row(ArrayView::from(&vec![text_length as i64]))?;
        }

        // job's done
        Ok(Self {
            texts: input.texts,
            tokens: input.tokens,
            entities: input.entities,
            num_words: input.num_words,
            num_tokens: max_tokens,
            input_ids,
            attention_masks,
            word_masks,
            text_lengths,            
        })
    }

}



/// Composable: Prompts => Encoded
pub struct PromptsToEncoded<'a, T> { 
    tokenizer: &'a T,
}

impl<'a, T> PromptsToEncoded<'a, T> {
    pub fn new(tokenizer: &'a T) -> Self {
        Self { tokenizer }
    }
}

impl<'a, T: Tokenizer> Composable<PromptInput, EncodedInput> for PromptsToEncoded<'a, T> {
    fn apply(&self, input: PromptInput) -> Result<EncodedInput> {
        EncodedInput::from(input, self.tokenizer)
    }
}


/// Unit tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() -> Result<()> {        
        let splitter = crate::text::splitter::RegexSplitter::default();        
        let tokenizer = crate::text::tokenizer::HFTokenizer::from_file("models/gliner_small-v2.1/tokenizer.json")?;
        let batch = [ "Short text", "This is a longer one, to test padding and gloubiboulga."];
        let entities = [ "Person", "Place" ];
        let input = super::super::text::TextInput::from_str(&batch, &entities)?;
        let tokenized = super::super::tokenized::TokenizedInput::from(input, &splitter, None)?;
        let prepared = PromptInput::from(tokenized);
        let encoded = EncodedInput::from(prepared, &tokenizer)?;
        // Some prints
        if false {
            println!("### {:?}", encoded.num_tokens);
            println!("Tokens: {:?}", encoded.input_ids);
            println!("Attn Masks: {:?}", encoded.attention_masks);
            println!("Word masks: {:?}", encoded.word_masks);
        }
        // Assertions on input ids
        const ENT_ID: i64 = 128002;
        const SEP_ID: i64 = 128003;
        assert_eq!(encoded.num_tokens, 22);
        let ids1 = encoded.input_ids.row(0);
        let ids2 = encoded.input_ids.row(1);
        assert_eq!(ids1.len(), encoded.num_tokens);
        assert_eq!(ids2.len(), encoded.num_tokens);
        assert_eq!(ids1.iter().filter(|id| **id == 0).count(), 13);
        assert_eq!(ids1.iter().filter(|id| **id == ENT_ID).count(), 2);
        assert_eq!(ids1.iter().filter(|id| **id == SEP_ID).count(), 1);
        assert_eq!(ids2.iter().filter(|id| **id == 0).count(), 0);
        assert_eq!(ids2.iter().filter(|id| **id == ENT_ID).count(), 2);
        assert_eq!(ids2.iter().filter(|id| **id == SEP_ID).count(), 1);
        // Assertions on attention mask
        let attn1 = encoded.attention_masks.row(0);
        let attn2 = encoded.attention_masks.row(1);
        assert_eq!(attn1.iter().filter(|id| **id == 1).count(), 9);
        assert_eq!(attn2.iter().filter(|id| **id == 1).count(), 22);
        // Everything rules
        Ok(())
    }

    #[test]
    fn test2() -> Result<()> {        
        let splitter = crate::text::splitter::RegexSplitter::default();        
        let tokenizer = crate::text::tokenizer::HFTokenizer::from_file(std::path::Path::new("models/gliner_small-v2.1/tokenizer.json"))?;
        let batch = [ "My name is James Bond", "I like to drive my Aston Martin"];
        let entities = [ "movie character", "vehicle" ];
        let input = super::super::text::TextInput::from_str(&batch, &entities)?;
        let tokenized = super::super::tokenized::TokenizedInput::from(input, &splitter, None)?;
        let prepared = PromptInput::from(tokenized);
        let encoded = EncodedInput::from(prepared, &tokenizer)?;
        // Some prints
        if false {
            println!("### {:?}", encoded.num_tokens);
            println!("Tokens: {:?}", encoded.input_ids);
            println!("Attn Masks: {:?}", encoded.attention_masks);
            println!("Word masks: {:?}", encoded.word_masks);
            println!("Text length: {:?}", encoded.text_lengths);
        }
        // Assertions on first sequence
        let ids1 = encoded.input_ids.row(0);
        let attn1 = encoded.attention_masks.row(0);
        let word1 = encoded.word_masks.row(0);
        let len1 = encoded.text_lengths.row(0);        
        assert_eq!(ids1.to_vec(), vec![1, 128002, 1421, 1470, 128002, 1508, 128003, 573, 601, 269, 1749, 8728, 2, 0, 0]);
        assert_eq!(attn1.to_vec(), vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0]);
        assert_eq!(word1.to_vec(), vec![0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 0, 0, 0]);
        assert_eq!(len1.to_vec(), vec![5]);
        // Assertions on second sequence
        let ids2 = encoded.input_ids.row(1);
        let attn2 = encoded.attention_masks.row(1);
        let word2 = encoded.word_masks.row(1);
        let len2 = encoded.text_lengths.row(1);
        assert_eq!(ids2.to_vec(), vec![1, 128002, 1421, 1470, 128002, 1508, 128003, 273, 334, 264, 1168, 312, 20844, 2963, 2]);
        assert_eq!(attn2.to_vec(), vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
        assert_eq!(word2.to_vec(), vec![0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 0]);
        assert_eq!(len2.to_vec(), vec![7]);
        Ok(())
    }

    #[test]
    fn test_multiword_entity_label() -> Result<()> {
        let splitter = crate::text::splitter::RegexSplitter::default();
        let tokenizer = crate::text::tokenizer::HFTokenizer::from_file("models/gliner_small-v2.1/tokenizer.json")?;
        let batch = [ "this is a test"];
        let entities = [ "multi label" ];
        let input = super::super::text::TextInput::from_str(&batch, &entities)?;
        let tokenized = super::super::tokenized::TokenizedInput::from(input, &splitter, None)?;
        let prepared = PromptInput::from(tokenized);
        let encoded = EncodedInput::from(prepared, &tokenizer)?;
        // Some prints
        if false {
            println!("### {:?}", encoded.num_tokens);
            println!("Tokens: {:?}", encoded.input_ids);
            println!("Attn Masks: {:?}", encoded.attention_masks);
            println!("Word masks: {:?}", encoded.word_masks);
        }
        // Assertions
        let ids = encoded.input_ids.row(0);
        assert_eq!(ids.len(), 10);
        let word_masks = encoded.word_masks.row(0);
        assert_eq!(word_masks.to_vec(), vec![0, 0, 0, 0, 0, 1, 2, 3, 4, 0]);
        // Everything rules
        Ok(())
    }

}
