import { HelpPopover } from "./HelpPopover";

/**
 * The `?` explanation of "Keep audio after processing", shared by the global setting and the
 * per-project choice so both say the same thing.
 */
export function AudioRetentionHelp() {
  return (
    <HelpPopover title="Keep audio after processing" recommended="Off (delete the audio)">
      <p>
        By default the recorded audio is deleted once the meeting has been processed successfully
        and its report is ready. This frees a lot of disk space.
      </p>
      <p>
        <strong>Deleted:</strong> the audio files of the meeting (and an imported audio or video
        file that was copied into the meeting folder).
      </p>
      <p>
        <strong>Kept:</strong> the transcript, the draft report, the key screenshots and the voice
        sample clips.
      </p>
      <p>
        Audio is never deleted while a meeting is still processing or if processing failed. Once it
        is deleted, you can still correct the transcript text, but you cannot process the meeting
        again from the audio.
      </p>
      <p>Keeping audio uses a lot of disk space: a 10 hour meeting takes several GB.</p>
    </HelpPopover>
  );
}
