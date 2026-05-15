use crate::jvm::{
    JVM,
    jvm_core::{HeapObject, JvmStackValue},
};

pub const CLASS_NAME: &str = "javax/microedition/media/Player";

/// Handles virtual method calls to the Player interface
pub fn handle_virtual_method(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    _args: &[JvmStackValue],
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("deallocate", "()V") => {
            // todo!("Player.deallocate not implemented yet");
            Ok(None)
        }
        ("realize", "()V") => {
            todo!("Player.realize not implemented yet");
            Ok(None)
        }
        ("prefetch", "()V") => {
            todo!("Player.prefetch not implemented yet");
            Ok(None)
        }
        ("start", "()V") => {
            todo!("Player.start not implemented yet");
            Ok(None)
        }
        ("stop", "()V") => {
            todo!("Player.stop not implemented yet");
            Ok(None)
        }
        ("close", "()V") => {
            // todo!("Player.close not implemented yet");
            Ok(None)
        }

        ("getMediaTime", "()J") => {
            // Returns current media time in milliseconds
            Ok(Some(JvmStackValue::Long(0)))
        }
        ("setMediaTime", "(J)V") => {
            // Sets the media time in milliseconds
            // For now, this is a no-op
            Ok(None)
        }

        // Duration query
        ("getDuration", "()J") => {
            // Returns total duration of media in milliseconds
            // Returns -1 if duration is unknown
            Ok(Some(JvmStackValue::Long(-1)))
        }

        // Looping
        ("setLoopCount", "(I)V") => {
            // Sets the number of times the media will be played
            // For now, this is a no-op
            Ok(None)
        }

        // Content type
        ("getContentType", "()Ljava/lang/String;") => {
            // Returns the content type of the media
            // For now, return empty string
            Ok(Some(JvmStackValue::String("".to_string())))
        }

        // Volume control (from Controllable)
        ("getControls", "()[Ljavax/microedition/media/Control;") => {
            // Returns array of available controls
            // For now, return null
            Ok(Some(JvmStackValue::Null))
        }

        ("getControl", "(Ljava/lang/String;)Ljavax/microedition/media/Control;") => {
            // Returns specific control by name
            // For now, return null
            Ok(Some(JvmStackValue::Null))
        }

        // State query (not part of interface but commonly needed)
        ("getState", "()I") => {
            // Returns current player state
            // 100 = UNREALIZED, 200 = REALIZED, 300 = PREFETCHED, 400 = STARTED, 500 = CLOSED
            // For now, return STARTED (400)
            Ok(Some(JvmStackValue::Int(400)))
        }

        _ => Err(format!(
            "Player.{}{} not implemented",
            method_name, descriptor
        )),
    }
}
